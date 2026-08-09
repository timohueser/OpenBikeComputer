//! The Map screen — the Riding view. The camera lives in [`AppState`](crate::AppState) (shared with
//! the host's pan/zoom); the screen itself holds only a [`MinuteTicker`] for the clock overlay. `draw`
//! renders the base map plus the route, travel chevrons, breadcrumb, user marker, and the map chrome:
//! floating top-centre clock digits, a bottom-centre one-slot warning chip, a bottom-left scale bar
//! (stepping up above the chip while one is up), a low-battery cue in the top-left corner, and the
//! pan HUD.
//!
//! Bindings depend on whether a ride is being tracked. Shared in Follow: up/down = zoom, `hold` =
//! enter Inspect, `back-hold` = Ride menu. Inspect keeps Back tap as the quick return to Follow:
//! `press` toggles Move ↔ Zoom, Back-hold toggles Route ↔ the last Free axis, and Select-hold only
//! toggles Free V ↔ Free H. Select-hold is inert in Zoom; Back-hold still switches Route/Free and
//! lands in Move. **Tracking** (the riding map): `press` = pause → Ride control,
//! `back` = the sibling Statistics view. **Not tracking** (the route-less browse map, reached from
//! the Menu's Map station): `press` = the start card, `back` = pop back to the Menu (there's no
//! Statistics sibling without a ride). Off-route chrome can't fire without a route, so the browse
//! map shows only clock / scale-bar / low-battery.

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

use crate::app::{Pan, PanBasis, PanTool};
use crate::input::Gesture;
use crate::settings::{DateTime, Units, WaypointMode};
use crate::wall_clock::MinuteTicker;
use crate::Msg;
use obc_ports::Fix;

use super::{Ctx, RenderFrame, Screen, ScreenTick, StatisticsScreen, Transition};

/// Fallback backdrop when a map carries no backdrop style.
const DEFAULT_BG_RGB565: u16 = 0x2104;

/// Stroke width (px) of the active-route overlay — bold enough to out-weigh the heaviest base road
/// (3 px), and sized so a direction chevron sits *inside* the line at riding zoom. `pub` so the
/// render benchmark's route scene pins its stroke weight to this exact value (re-exported as
/// [`crate::screen::ROUTE_WEIGHT`]).
pub const ROUTE_WEIGHT: u32 = 11;
/// Narrower stroke painted inside the route line for the chooser's to-be-skipped stretch. The
/// magenta edges remain visible, while warning-orange makes the selected interval unmistakable.
const SKIPPED_WEIGHT: u32 = 7;

/// Colour of the route direction chevrons — white, for contrast over the magenta route line. Drawn
/// only at riding zoom (see [`CHEVRON_MAX_MPP`]).
const ARROW_COLOR: u16 = super::palette::PARCHMENT;

/// Zoom threshold (ground meters per pixel) at/below which the chevrons are drawn — roughly riding
/// scale — fading out on wider overviews. A scale gate, independent of the map's LOD pyramid.
const CHEVRON_MAX_MPP: f32 = 4.0;

/// Stroke width (px) of the breadcrumb — thinner than the route, so the route stays dominant where
/// the two coincide.
const BREADCRUMB_WEIGHT: u32 = 3;

/// Half-diagonal (px) of a waypoint diamond — a ~9 px point-to-point ink rhombus, small map furniture
/// on the route line (epic #523, part 2). No zoom gate (unlike the chevrons' [`CHEVRON_MAX_MPP`]):
/// the resident table is ≤ `MAX_WAYPOINTS`, so even a wide overview shows only a calm handful of
/// anchors — the "day at a glance" read.
const WAYPOINT_DIAMOND_R: i32 = 4;

/// The live map / Follow view. The camera is the shared [`AppState`](crate::AppState); the only
/// screen-local state is the clock overlay's [`MinuteTicker`], which fires a region-clipped repaint
/// of the clock digits once each minute the wall clock rolls over.
#[derive(Debug, Default)]
pub struct MapScreen {
    /// Fires a repaint of the clock digits each minute the wall clock rolls over (see
    /// [`tick_timers`](MapScreen::tick_timers)) so `HH:MM` advances without a full map redraw.
    ticker: MinuteTicker,
    /// The route-less browse map's one-shot "press to start a ride" hint (T6, #684) — shown on
    /// entry, auto-hidden after [`HINT_MS`]. A riding map's copy stays [`BrowseHint::Done`].
    hint: BrowseHint,
}

/// How long the browse-map start hint stays up after entry, in milliseconds.
const HINT_MS: u32 = 4_000;

/// The route-less browse map's one-shot start-hint chip state (T6, #684). A fresh `MapScreen`
/// starts [`Fresh`](BrowseHint::Fresh); its first [`tick`](BrowseHint::tick) classifies it — a
/// browse map (not tracking) starts the timer and shows the chip, a riding map goes straight to
/// [`Done`](BrowseHint::Done) so the chip never shows there. Re-entering the browse map is a fresh
/// `MapScreen`, so the hint returns; a pop back from the start card is the same instance, so it
/// doesn't reappear once expired.
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
    /// Advance the hint one poll against `now_ms`. Arms on the first poll of a browse map (`!tracking`),
    /// suppresses on a riding map, and expires [`HINT_MS`] after arming. Returns
    /// `(changed, next_wake_ms)`: `changed` is the single repaint that clears the chip on expiry; the
    /// residual wake keeps an event-driven host armed to fire it from warm sleep.
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

    /// Whether the chip draws this frame — up while `Fresh` (the entry frame before the first tick)
    /// or `Showing`. The caller still gates on `!tracking` / `!panning` / no higher-priority chip.
    fn chip_up(self) -> bool {
        matches!(self, BrowseHint::Fresh | BrowseHint::Showing(_))
    }
}

impl MapScreen {
    pub fn new() -> Self {
        MapScreen::default()
    }

    /// Poll the clock overlay's minute tick — the Map's half of the screens' timed
    /// [`tick_timers`](super::Screen::tick_timers) contract. When the clock is **visible** (the
    /// `Clock on map` setting is on and we're not panning — the pan chevron owns the top-centre slot),
    /// a minute rollover self-dirties just the [`clock_region`], and the host clips the repaint
    /// to it (the region path from #500/#513) so the map plane isn't re-rendered. When the pill is
    /// **hidden** the minute wake is not armed at all — a parked map isn't woken to no purpose.
    #[allow(clippy::too_many_arguments)] // two timed tenants (clock overlay + browse hint) in one poll
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
        // Clock overlay: a minute rollover self-dirties just the pill region, armed only while the
        // pill is visible (setting on, not panning, a frame drawn). Hidden, still observe the minute
        // so a later show doesn't fire a stale rollover.
        let (clk_changed, clk_wake, clk_region) = if !map_clock || pan_active || w == 0 {
            let _ = self.ticker.changed(now);
            (false, None, None)
        } else {
            (self.ticker.changed(now), Some(ms_to_next_minute), Some(clock_region(w)))
        };
        // Browse-map start hint (T6 #684): the one-shot bottom chip, armed on entry and expiring
        // HINT_MS later. Its expiry repaints the chip band + steps the scale bar back down, so it's a
        // full-frame change (region `None`) — unlike the clock's region-clipped digit tick.
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
        // Pan mode is a sub-mode of the Map: while the shared camera holds a `pan`,
        // Select/Back drive panning instead of the Follow bindings below.
        if cx.state.pan.is_some() {
            return handle_pan(g, cx);
        }
        match g {
            Gesture::Step(n) => {
                cx.state.zoom_step(n);
                Transition::None
            }
            // hold = enter Pan mode: the camera detaches and the pan HUD appears.
            Gesture::Hold => {
                cx.state.enter_pan(cx.activity.active_route.is_some(), cx.activity.progress_m);
                Transition::None
            }
            // `back`: while tracking, swap to the sibling Statistics view (its `back` swaps straight
            // back here — the Map↔Statistics ring only exists mid-ride). On the route-less *browse*
            // map (not tracking), there's no sibling to swap to, so `back` pops back to the Menu.
            Gesture::Back if cx.activity.is_tracking() => {
                Transition::Replace(Screen::Statistics(StatisticsScreen::new()))
            }
            Gesture::Back => Transition::Pop,
            // `press`: while tracking, pause → Ride control (the shared riding binding). On the
            // browse map, open the small start card instead of the Paused page.
            Gesture::Press if !cx.activity.is_tracking() => {
                Transition::Push(Screen::RideStart(super::RideStartScreen::new()))
            }
            Gesture::Press | Gesture::BackHold => super::riding_common(g, cx),
        }
    }

    pub fn draw<D, F, S>(&self, cv: &mut Canvas<D, F>, rx: &mut RenderFrame<'_, S>)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: obc_map_scene::MapScene,
    {
        let vp = rx.state.viewport(rx.w as f32, rx.h as f32);
        let Some(marker565) = draw_map_scene(cv, rx, &vp, None) else { return };

        // The remaining chrome draws in the palette vocabulary, back through the canvas.
        let panning = rx.state.pan.is_some();

        // Low-battery cue (top-left corner): a small warning-red battery glyph only when the charge
        // has dropped below LOW_BATTERY_PCT — nothing above it, so there's no permanent map battery
        // indicator. Shown in pan mode too (the top-right corner belongs to the pan compass rose).
        if rx.state.battery_pct < LOW_BATTERY_PCT {
            draw_low_battery(cv);
        }

        // Clock (top-centre): a floating HH:MM — bare ink digits with a 1 px parchment halo, no
        // pill, so it informs without drawing the eye. Shown when the setting is on. Hidden while
        // panning — the pan HUD's top chevron / compass own the top edge — so it never fights the
        // chevron; `tick_timers` mirrors that gate when arming the minute wake.
        if rx.settings.map_clock && !panning {
            draw_clock(cv, rx.w, rx.now);
        }

        // Bottom-centre one-slot warning chip, shown only when there's something to say. "No GPS
        // Fix" takes priority over off-route: with no fix the match is stale, so cross-track
        // distance is meaningless. Suppressed while panning — the pan HUD's bottom chevron owns the
        // bottom-centre slot (they'd collide), and panning is deliberate map inspection anyway; the
        // chip returns the moment pan exits.
        let warning_up = !panning && (rx.no_fix || rx.activity.off_route);
        if warning_up {
            if rx.no_fix {
                draw_status_chip(cv, rx.w, rx.h, rx.t(Msg::MapNoGpsFix));
            } else {
                let mut s: heapless::String<20> = heapless::String::new();
                super::write_off_route(&mut s, rx.t(Msg::MapOffRoute), rx.activity.dist_to_route_m, rx.settings.units);
                draw_status_chip(cv, rx.w, rx.h, &s);
            }
        }

        // Waypoint chip (same bottom-centre slot): the calm `◆ NAME  <dist>` pill counting the
        // along-route distance to the next named waypoint, governed by the `WaypointMode` setting.
        // The warning chip keeps slot priority — the pure helper below only reports a chip when the
        // warning chip is down (and not panning), so the two never collide.
        let wpt_chip = waypoint_chip(
            rx.settings.waypoint_mode,
            panning,
            rx.no_fix,
            rx.activity.off_route,
            rx.activity.next_waypoint,
            rx.waypoints.as_slice(),
            rx.activity.progress_m,
        );
        if let Some((k, dist_to_go)) = wpt_chip {
            let dist = crate::stat_fields::fmt_dist_short(dist_to_go, rx.settings.units);
            draw_waypoint_chip(cv, rx.w, rx.h, rx.waypoints.as_slice()[k].name.as_str(), &dist);
        }

        // Browse-map start hint (T6 #684): the lowest-priority bottom chip — `Press to start a ride`
        // on the route-less browse map, shown on entry and auto-hidden after 4 s (the `hint` timer).
        // Never while tracking or panning, and dropped whenever a warning / waypoint chip wants the
        // slot (so it never collides). Its own timer drives the auto-hide; this only reads its state.
        let hint_up =
            !rx.activity.is_tracking() && !panning && !warning_up && wpt_chip.is_none() && self.hint.chip_up();
        if hint_up {
            draw_hint_chip(cv, rx.w, rx.h, rx.t(Msg::MapPressToStart));
        }

        // Scale bar (bottom-left): the largest round distance that fits the target on-screen width
        // at the current zoom, in the units setting's system. Right in the corner — except while a
        // bottom chip is up (warning **or** waypoint), when it steps to just above the chip band so
        // a wide chip ("off route 153km", "◆ Pass Summit  0.4km") never runs under it. Visible in
        // pan mode too (where it's most useful) — the pan HUD's bottom chevron is centred, well
        // clear of the corner.
        // …stepped above whichever bottom chip is up. The hint's band is taller (two lines), so pass
        // its band height rather than a bare bool, so a wide scale bar never runs under it.
        let chip_band = if warning_up || wpt_chip.is_some() {
            CHIP_H
        } else if hint_up {
            hint_band_h(rx.t(Msg::MapPressToStart))
        } else {
            0
        };
        if rx.settings.map_scale_bar {
            draw_scale_bar(cv, rx.h, chip_band, vp.meters_per_pixel(), rx.settings.units);
        }

        // Pan-mode HUD. Drawn last so it sits over the map + marker, and only while panning.
        if let Some(pan) = rx.state.pan {
            draw_pan_hud(cv, (rx.w as f32, rx.h as f32), pan, rx.state.user_fix, marker565, &vp);
        }
    }
}

/// Optional detour ink added to the shared map scene (#882): the skipped-span interval and rejoin
/// candidate the chooser owns, plus (on the preview screen) the planned detour's decimated
/// polyline. The normal Map passes `None` and pays no extra route decode.
#[derive(Clone, Copy)]
pub(crate) struct DetourMapOverlay<'a> {
    pub start_m: u32,
    pub end_m: u32,
    pub candidate: (i32, i32),
    /// The planned detour's decimated polyline — empty on the chooser (nothing planned yet).
    pub detour: &'a [(i32, i32)],
}

/// Draw the reusable map scene (base map, full route, optional skipped-range + detour ink,
/// breadcrumb, waypoints, rider and candidate). Map chrome stays in [`MapScreen::draw`]; the
/// Detour chooser/preview add their own floating HUD after this returns.
pub(crate) fn draw_map_scene<D, F, S>(
    cv: &mut Canvas<D, F>,
    rx: &mut RenderFrame<'_, S>,
    vp: &Viewport,
    skip: Option<DetourMapOverlay<'_>>,
) -> Option<u16>
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
    S: obc_map_scene::MapScene,
{
    let scene = rx.scene?;
    let rx = &mut rx.render;
    // The only reader of the host's lent scratch (#1146 P2) — which is why every other screen may
    // be drawn with `None`. Reaching here without one means the host called a render entry point
    // with `None` under a map-drawing base: skip the map rather than invent one, and say so loudly
    // in a debug build.
    let Some(scratch) = rx.scratch.as_deref_mut() else {
        debug_assert!(false, "a map-drawing base needs the host's RenderScratch, but none was lent");
        return None;
    };
    let bg565 = scene.backdrop_style().map_or(DEFAULT_BG_RGB565, |style| style.color);
    let (target, color_fn) = cv.split();
    let bg = color_fn(bg565);
    // #1096 (provisional): the rider's contour switch, restated every frame from `Settings` — the
    // switch lives there, not in the scratch (#1146's Config/Scratch rule), so a flip on the Display
    // screen lands on the very next map frame with no reload and nothing to reset. Suppression drops
    // the terrain layer in the collect pass, so nothing is decoded, let alone drawn.
    let cfg = obc_render::RenderConfig { terrain_layer: rx.settings.map_contours };
    // The rain overlay lease (WX10), taken for this frame: precipitation draws inside the base-map
    // paint order (below the road band), so roads, route, rider and the chrome below all stay
    // above it. `None` — no store, nothing current, expired — renders the byte-identical
    // rain-free map.
    let rain = rx.rain.take();
    let mut stats = scratch.render_rain_timed(target, scene, vp, bg, cfg, rain, color_fn, rx.clock);
    let arrows_at = (skip.is_none() && vp.meters_per_pixel() <= CHEVRON_MAX_MPP).then_some(rx.activity.progress_m);

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
            // The planned detour (#882): blue, so the replanned portion reads apart from the
            // magenta route it will replace and the warning-colored span it avoids.
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
        let trail = color_fn(super::palette::BREADCRUMB);
        scratch.stroke_path(target, vp, rx.breadcrumb.points(), trail, BREADCRUMB_WEIGHT);
    }
    rx.stats = stats;

    draw_waypoint_diamonds(cv, vp, rx.waypoints.as_slice(), rx.w, rx.h);
    let marker565 = if rx.activity.off_route { super::palette::WARNING } else { scene.marker_color() };
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
    Some(marker565)
}

// ---- Waypoint diamonds (on the route line) --------------------------------

/// The four screen vertices `(top, bottom, left, right)` of a waypoint diamond centred at
/// `(cx, cy)`, or `None` when the centre lies more than one half-diagonal ([`WAYPOINT_DIAMOND_R`])
/// outside the `w`×`h` panel — the off-panel cull. A diamond straddling an edge (centre within the
/// margin) still draws; one wholly past it is dropped. Pure integer geometry, unit-tested below.
fn waypoint_diamond(cx: i32, cy: i32, w: i32, h: i32) -> Option<(Point, Point, Point, Point)> {
    let r = WAYPOINT_DIAMOND_R;
    if cx < -r || cx > w + r || cy < -r || cy > h + r {
        return None;
    }
    Some((Point::new(cx, cy - r), Point::new(cx, cy + r), Point::new(cx - r, cy), Point::new(cx + r, cy)))
}

/// Draw the route's named waypoints as small filled-[`INK`](super::palette::INK) diamonds at each
/// entry's own (`lon`, `lat`) — the stored coordinate, **not** snapped to the polyline (it may sit
/// slightly off it). Each diamond is two [`triangle`](Surface::triangle)s (top + bottom halves
/// sharing the left/right vertices), the way the rest of the map chrome draws. The table is empty
/// with no route loaded, so this is a no-op then.
fn draw_waypoint_diamonds(cv: &mut impl Surface, vp: &Viewport, wpts: &[WptEntry], w: i32, h: i32) {
    for wp in wpts {
        let (sx, sy) = vp.to_screen(wp.lon, wp.lat);
        if let Some((top, bottom, left, right)) = waypoint_diamond(sx, sy, w, h) {
            cv.triangle(top, left, right, super::palette::INK);
            cv.triangle(bottom, left, right, super::palette::INK);
        }
    }
}

/// Pan-mode gesture bindings, active while [`AppState::pan`](crate::AppState::pan) is `Some`.
/// Up/Down applies the active Move/Zoom tool; `press` toggles those tools; Back-hold toggles the
/// Route/Free family and Select-hold changes only an active Free axis. Select-hold is inert in Zoom;
/// Back-hold switches family and lands in Move. `back` exits to Follow. The holds override the
/// riding map's normal long-press actions in Inspect.
pub(super) fn handle_pan(g: Gesture, cx: &mut Ctx) -> Transition {
    let has_route = cx.activity.active_route.is_some();
    match g {
        Gesture::Step(n) => cx.state.pan_step(n, cx.activity.route_total_m),
        Gesture::Press => cx.state.toggle_pan_tool(),
        Gesture::Hold => cx.state.toggle_pan_free_axis(),
        Gesture::Back => cx.state.exit_pan(),
        Gesture::BackHold => cx.state.toggle_pan_family(has_route),
    }
    Transition::None
}

/// A compact **bottom-centre** status chip ("No GPS Fix", "off route NNNm") — the one warning slot
/// on the map, kept away from the top's clock + low-battery chrome. The caller owns the priority
/// rule of what to say; the chip vanishes the moment there's nothing to report. Warning-orange, so
/// it reads as an alert (the quieter clock uses bare ink).
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
/// Visible padding (px) between the pill's edge and the text block's cap extents — the **same**
/// above the first line and below the last, by construction: the pill height is derived from the
/// wrapped line count (owner review round 1 — the old fixed 50 px pill left the second line ~2 px
/// off the bottom edge while 11 px hung over the top).
const HINT_PAD_Y: i32 = 8;
/// Terminus [`Font::Label`] glyphs ink from 4 px below their cell-top anchor (the face's top
/// bearing, measured on a rendered frame). The centering math offsets the line anchors by it so the
/// *visible* text block sits symmetric in the pill — not the padded glyph-cell box.
const HINT_TEXT_BEARING_Y: i32 = 4;

/// The hint pill's height for `lines` wrapped [`Font::Label`] lines: the visible text block
/// (cap-top of the first line to cap-bottom of the last) plus the symmetric [`HINT_PAD_Y`].
fn hint_chip_h(lines: i32) -> i32 {
    (lines - 1) * HINT_LINE_PITCH + Font::Label.cap_height() as i32 + 2 * HINT_PAD_Y
}

/// The hint pill's band height for the (language-dependent) hint copy `s` — [`hint_chip_h`] over
/// the wrapped line count. Shared by [`draw_hint_chip`] and the scale bar's step-up, so the bar
/// clears exactly the pill that draws.
fn hint_band_h(s: &str) -> i32 {
    hint_chip_h(if wrap2(s).1.is_empty() { 1 } else { 2 })
}

/// The browse-map **start hint** pill (T6 #684): calm ink on parchment — warning-orange stays
/// reserved for the alert chip, matching the muted clock — at [`Font::Label`], the sentence wrapped
/// to two centred lines (the full `Press to start a ride` cannot fit one line at 240 px in even the
/// smallest font). The pill height derives from the wrapped line count and the text block centres
/// in it (see [`HINT_PAD_Y`]). Same rounded bottom-centre pill idiom as [`draw_status_chip`], just
/// taller. Lowest chip priority; the caller only reaches here when no warning / waypoint chip is up.
fn draw_hint_chip(cv: &mut impl Surface, w: i32, h: i32, s: &str) {
    use super::palette::*;
    let font = Font::Label;
    let (l1, l2) = wrap2(s);
    let ph = hint_band_h(s);
    let tw = (text_width(l1, font) as i32).max(text_width(l2, font) as i32);
    let pw = tw + 16;
    let px = (w - pw) / 2;
    let py = h - ph - CHIP_MARGIN;
    cv.round(rect(px, py, pw, ph), 9, PARCHMENT);
    cv.round_outline(rect(px, py, pw, ph), 9, INK);
    // First line's anchor: the visible cap top lands exactly HINT_PAD_Y under the pill edge.
    let ty = py + HINT_PAD_Y - HINT_TEXT_BEARING_Y;
    cv.text(l1, Point::new(w / 2, ty), font, TextAlign::Center, INK);
    if !l2.is_empty() {
        cv.text(l2, Point::new(w / 2, ty + HINT_LINE_PITCH), font, TextAlign::Center, INK);
    }
}

/// Split `s` into two balanced centred lines for the hint pill: pick the word break (space) whose
/// resulting first line is closest to half the string (in characters), so neither line orphans a
/// single word. A string with no space falls through as one line (never happens for the catalog
/// copy). Char-count balance is a fine proxy for width here — the font is monospace.
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

/// The status chip's band height and its inset from the bottom edge (above the panel frame, below
/// where the pan bottom chevron would draw — the two never coexist; the chip is pan-suppressed).
const CHIP_H: i32 = 36;
const CHIP_MARGIN: i32 = 10;

// ---- Waypoint chip (bottom-centre) ----------------------------------------

/// Approach radius (metres): in [`WaypointMode::Approach`] the chip appears once the next waypoint
/// is within this along-route distance ahead and counts down to it. `pub(crate)` so the setting's
/// doc + the tests share the one value.
pub(crate) const WAYPOINT_APPROACH_M: u32 = 500;

/// Half-diagonal (px) of the chip's ink diamond glyph — part 2's route diamond at chip scale.
const WPT_CHIP_DIAMOND_R: i32 = 4;
/// Horizontal inset (px) the pill may reach from each screen edge. Tighter than the status chip's
/// centred band (#688): the name gets the full remaining width so a long-but-common label like
/// `Pass Summit` reads whole beside its distance at 240 px, rather than truncating with slack.
const WPT_CHIP_INSET_X: i32 = 2;
/// Horizontal pad inside the pill (each side).
const WPT_CHIP_PAD_X: i32 = 4;
/// Gap between the diamond glyph and the name.
const WPT_CHIP_GAP_D: i32 = 3;
/// Gap between the name and the right-aligned distance — the one "standard gap" the name budget
/// reserves before the fixed-width distance.
const WPT_CHIP_GAP_N: i32 = 4;

/// Whether the Map waypoint chip shows this frame, and — if so — which resident waypoint it names
/// and the along-route distance-to-go it reads. A **pure** helper (no render context) so the one
/// visibility rule is unit-tested directly. Returns `Some((index, dist_to_go_m))` when the chip is
/// up (the caller pairs `index` with `wpts[index].name`), or `None` when it stays down.
///
/// Shows iff **all** of: not `panning`; the warning chip is **down** (`!no_fix && !off_route` — it
/// keeps slot priority, and both its states also make the along-route distance stale/meaningless);
/// `next_waypoint` is `Some(k)` and in range of `wpts`; and the `mode` allows — [`Always`] always,
/// [`Approach`] only within [`WAYPOINT_APPROACH_M`], [`Off`] never. `dist_to_go` is
/// `wpts[k].dist_along_m.saturating_sub(progress_m)`, so it clamps to `0` during the 100 m
/// pass-linger (the wanted "you are here" readout until the index advances).
///
/// [`Always`]: WaypointMode::Always
/// [`Approach`]: WaypointMode::Approach
/// [`Off`]: WaypointMode::Off
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

/// Fit `name` into `budget_px` at [`Font::Body`], dropping trailing chars and appending a two-dot
/// ASCII ellipsis (`..` — the device font is printable-ASCII only, so `…` would render as tofu) when
/// it genuinely overflows. Writes the result into `buf` and returns it. Pure integer geometry over
/// the monospace cell width, so the truncation is deterministic and testable.
fn fit_name<'b>(name: &str, budget_px: i32, buf: &'b mut heapless::String<28>) -> &'b str {
    buf.clear();
    let char_w = Font::Body.char_width() as i32;
    let chars = name.chars().count() as i32;
    if chars * char_w <= budget_px {
        let _ = buf.push_str(name); // fits whole (name ≤ WAYPOINT_NAME_CAP bytes ≤ buf)
        return buf.as_str();
    }
    const ELL: &str = "..";
    let ell_w = text_width(ELL, Font::Body) as i32;
    let keep = ((budget_px - ell_w) / char_w).max(0) as usize;
    for ch in name.chars().take(keep) {
        if buf.push(ch).is_err() {
            break;
        }
    }
    let _ = buf.push_str(ELL);
    buf.as_str()
}

/// Draw the calm bottom-centre waypoint pill: `◆ NAME  <dist>` in [`INK`](super::palette::INK) on
/// parchment (warning-orange stays reserved for the alert chip, matching the muted clock). Same
/// pill geometry as [`draw_status_chip`]; a filled ink diamond at the left, the (truncated-to-fit)
/// name, and the right-aligned distance. The whole pill is kept within `w − 2·WPT_CHIP_INSET_X` by
/// shrinking the name only — the distance is measured first and never truncated.
fn draw_waypoint_chip(cv: &mut impl Surface, w: i32, h: i32, name: &str, dist: &str) {
    use super::palette::*;
    let font = Font::Body;
    let diamond_w = 2 * WPT_CHIP_DIAMOND_R + 1;
    let dist_w = text_width(dist, font) as i32;
    // Everything but the name is fixed; the name gets whatever remains inside the max pill width.
    let fixed_w = 2 * WPT_CHIP_PAD_X + diamond_w + WPT_CHIP_GAP_D + WPT_CHIP_GAP_N + dist_w;
    let name_budget = (w - 2 * WPT_CHIP_INSET_X) - fixed_w;
    let mut buf = heapless::String::<28>::new();
    let name = fit_name(name, name_budget, &mut buf);
    let name_w = text_width(name, font) as i32;

    let pw = fixed_w + name_w;
    let px = (w - pw) / 2;
    let py = h - CHIP_H - CHIP_MARGIN;
    cv.round(rect(px, py, pw, CHIP_H), 9, PARCHMENT);
    cv.round_outline(rect(px, py, pw, CHIP_H), 9, INK);

    // Ink diamond, vertically centred — two triangles sharing the left/right vertices (part 2's idiom).
    let dcx = px + WPT_CHIP_PAD_X + WPT_CHIP_DIAMOND_R;
    let dcy = py + CHIP_H / 2;
    let r = WPT_CHIP_DIAMOND_R;
    let (left, right) = (Point::new(dcx - r, dcy), Point::new(dcx + r, dcy));
    cv.triangle(Point::new(dcx, dcy - r), left, right, INK);
    cv.triangle(Point::new(dcx, dcy + r), left, right, INK);

    // Name after the diamond (left-aligned), distance at the pill's right pad (right-aligned). Text
    // top at `py + 5` centres Body in the 36 px band, matching `draw_status_chip`.
    let ty = py + 5;
    let name_x = px + WPT_CHIP_PAD_X + diamond_w + WPT_CHIP_GAP_D;
    cv.text(name, Point::new(name_x, ty), font, TextAlign::Left, INK);
    cv.text(dist, Point::new(px + pw - WPT_CHIP_PAD_X, ty), font, TextAlign::Right, INK);
}

// ---- Clock (top-centre) ---------------------------------------------------

/// Top inset of the floating `HH:MM` digits.
const CLOCK_TOP: i32 = 8;

/// The rectangle the top-centre clock digits occupy, in panel pixels — the dirty region
/// [`tick_timers`](MapScreen::tick_timers) reports so the host clips the minute repaint to just the
/// digits instead of re-rendering the whole map plane. Sized for a fixed 5-glyph `HH:MM` in
/// [`Font::Body`] — constant, so the region doesn't shift as the digits change (`11` vs `22`) —
/// with two pixels of margin all round covering the halo strokes.
pub fn clock_region(w: i32) -> Rectangle {
    let tw = text_width("00:00", Font::Body) as i32;
    let th = Font::Body.line_height() as i32;
    Rectangle::new(Point::new((w - tw) / 2 - 2, CLOCK_TOP - 2), Size::new(tw as u32 + 4, th as u32 + 4))
}

/// Draw the top-centre `HH:MM` clock: bare ink digits floating on the map — no pill, no white
/// backing, just the scale-bar label's 1 px parchment halo so they stay readable over dark terrain.
/// One font step up from the rest of the map chrome ([`Font::Body`]) so the time reads at a glance;
/// deliberately *muted* otherwise (the warning-orange of [`draw_status_chip`] stays reserved for
/// alerts). "Small and simple, just readable."
fn draw_clock(cv: &mut impl Surface, w: i32, now: DateTime) {
    use super::palette::*;
    let mut s: heapless::String<8> = heapless::String::new();
    let _ = write!(s, "{:02}:{:02}", now.hour, now.minute);
    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        cv.text(&s, Point::new(w / 2 + dx, CLOCK_TOP + dy), Font::Body, TextAlign::Center, PARCHMENT);
    }
    cv.text(&s, Point::new(w / 2, CLOCK_TOP), Font::Body, TextAlign::Center, INK);
}

// ---- Low-battery cue (top-left corner) -----------------------------------

/// Battery percentage below which the top-left warning glyph appears. At/above it the map shows no
/// battery indicator at all.
const LOW_BATTERY_PCT: u8 = 10;

/// Draw the low-battery cue: a small warning-red battery silhouette in the top-left corner (a
/// scaled-down cousin of the Home gauge's shell). Filled solid red — this is the "act now" state, not
/// a level readout — with an ink halo behind it so it reads over any terrain.
fn draw_low_battery(cv: &mut impl Surface) {
    use super::palette::*;
    let (x, y) = (10, 10);
    let (bw, bh, nub) = (26, 13, 3);
    // Ink halo (the shell + nub grown by 1px) so it reads over any map colour.
    cv.round_outline(rect(x - 1, y - 1, bw + 2, bh + 2), 3, INK);
    cv.round_outline(rect(x, y, bw, bh), 3, WARNING);
    cv.round(rect(x + bw, y + bh / 3, nub, bh / 3), 1, WARNING);
    // A solid red core inside the shell — the alert fill.
    cv.round(rect(x + 3, y + 3, bw - 6, bh - 6), 1, WARNING);
}

// ---- Scale bar (bottom-left) ---------------------------------------------

/// Largest on-screen width (px) the scale bar may reach — the chosen round distance is the biggest
/// `1/2/5 × 10ⁿ` that fits inside it (~⅓ of the 240px panel), long enough to read but short enough to
/// clear the pan HUD's centred bottom chevron. The 1/2/5 steps keep the realised bar within ~40–90 px.
const SCALE_TARGET_MAX_PX: f32 = 90.0;
/// The scale bar's left inset and the tick half-height.
const SCALE_MARGIN_X: i32 = 12;
/// Baseline inset from the bottom edge — right in the corner normally, stepped up past the chip
/// band (its height + inset + a gap) while a bottom chip is up.
const SCALE_MARGIN_Y: i32 = 12;
const SCALE_TICK_H: i32 = 5;

/// Draw the scale bar at the bottom-left: a horizontal ink line with end ticks and a length label,
/// haloed in parchment so it reads over terrain. The distance is the largest 1/2/5 × 10ⁿ that fits
/// [`SCALE_TARGET_MIN_PX`]..[`SCALE_TARGET_MAX_PX`] at the current `mpp` (metres per pixel), in the
/// `units` system.
fn draw_scale_bar(cv: &mut impl Surface, h: i32, chip_band: i32, mpp: f32, units: Units) {
    use super::palette::*;
    let Some((bar_px, label)) = scale_bar_choice(mpp, units) else {
        return; // a degenerate zoom (non-finite mpp) — draw nothing rather than a bogus bar
    };
    let x0 = SCALE_MARGIN_X;
    let x1 = x0 + bar_px;
    // In the corner normally; stepped above the bottom chip band (its height + inset + a gap) when a
    // chip is up (`chip_band > 0`) — a taller band (the two-line hint) steps the bar proportionally.
    let y = h - if chip_band > 0 { chip_band + CHIP_MARGIN + 12 } else { SCALE_MARGIN_Y };
    // Parchment halo: the same strokes one pixel thicker/offset, drawn first.
    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        cv.line(Point::new(x0 + dx, y + dy), Point::new(x1 + dx, y + dy), PARCHMENT);
        cv.line(Point::new(x0 + dx, y - SCALE_TICK_H + dy), Point::new(x0 + dx, y + dy), PARCHMENT);
        cv.line(Point::new(x1 + dx, y - SCALE_TICK_H + dy), Point::new(x1 + dx, y + dy), PARCHMENT);
    }
    // The ink bar: baseline + the two end ticks.
    cv.line(Point::new(x0, y), Point::new(x1, y), INK);
    cv.line(Point::new(x0, y - SCALE_TICK_H), Point::new(x0, y), INK);
    cv.line(Point::new(x1, y - SCALE_TICK_H), Point::new(x1, y), INK);
    // The label sits just above the bar, left-aligned to its start. Halo it too, so it reads on terrain.
    let ly = y - SCALE_TICK_H - Font::Label.line_height() as i32 - 1;
    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        cv.text(&label, Point::new(x0 + dx, ly + dy), Font::Label, TextAlign::Left, PARCHMENT);
    }
    cv.text(&label, Point::new(x0, ly), Font::Label, TextAlign::Left, INK);
}

/// The nice 1/2/5 mantissa steps a scale bar chooses from, largest-first — the classic map-scale
/// progression (…, 500, 200, 100, 50, 20, 10, …).
const NICE_STEPS: [u32; 3] = [5, 2, 1];

/// The largest `1/2/5 × 10ⁿ` at or below `max`, or `None` below 1 — the classic scale-bar rounding.
/// Bounded loop (no libm log): at most a handful of decades across the whole zoom range.
fn nice_125(max: f32) -> Option<u32> {
    if max < 1.0 {
        return None; // sub-unit scale — no sensible round value
    }
    // Walk powers of ten up to the 10ⁿ at or just below `max`, then try 5·10ⁿ, 2·10ⁿ, 1·10ⁿ from the
    // decade above down, taking the first (largest) that fits.
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

/// Pick the scale bar's `(pixel width, label)` for the current `mpp` and unit system, or `None` for a
/// non-finite / non-positive `mpp` (a degenerate camera). The rule: the **largest** round distance
/// `1/2/5 × 10ⁿ` — in the display unit the label will use: metres/kilometres, or feet *below* a mile
/// and whole miles above it (a bar says "2mi", never the feet-rounded "1.8mi") — whose on-screen
/// width is at most [`SCALE_TARGET_MAX_PX`]. The distance math is derived straight from `mpp` (the
/// render transform's metres-per-pixel), so the bar can never disagree with the map's true scale.
fn scale_bar_choice(mpp: f32, units: Units) -> Option<(i32, heapless::String<8>)> {
    if !(mpp.is_finite() && mpp > 0.0) {
        return None;
    }
    // Work in the display unit's base: metres (metric) or feet (imperial). `unit_per_px` is how many
    // of that base unit one screen pixel spans.
    let unit_per_px = if units.is_imperial() { mpp * crate::settings::FT_PER_M } else { mpp };
    // The largest base-unit distance that still fits the target width.
    let max_dist = SCALE_TARGET_MAX_PX * unit_per_px;
    // Imperial rounds in 1/2/5 miles once a mile fits; in feet below that. Metric in metres/km.
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

/// Format a chosen scale distance (in the display base unit — metres or feet) as its bar label:
/// `NNNm` / `N.Nkm` / `NNkm` in metric, `NNNft` / `N.Nmi` / `NNmi` in imperial. The `1/2/5` values
/// keep the kilo/mile forms to at most one decimal.
fn scale_label(dist: u32, units: Units) -> heapless::String<8> {
    use crate::settings::FT_PER_MI;
    let mut s: heapless::String<8> = heapless::String::new();
    if units.is_imperial() {
        if dist >= FT_PER_MI {
            // Whole miles for the round values that land there (5280·1 = 1mi, ·2, ·5…); the sub-mile
            // 1/2/5×10ⁿ feet (2000, 5000) show a single decimal.
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

/// Pan-mode HUD geometry — every tunable pixel size in one place. (The camera-travel-per-step
/// knob lives with the pan logic as [`crate::app::PAN_STEP_PX`].)
mod hud {
    /// Active-axis chevron — an open, round-capped "Λ" caret: tip `REACH` ahead of the centre, back
    /// corners `BACK` behind ± `SPREAD`, stroked at half-width `HW`, inset `INSET` from the edge.
    pub const CHEV_REACH: f32 = 9.0;
    pub const CHEV_BACK: f32 = 1.0;
    pub const CHEV_SPREAD: f32 = 10.0;
    pub const CHEV_HW: f32 = 2.5;
    pub const CHEV_INSET: f32 = 20.0;
    /// Ink halo thickness drawn behind every glyph so it reads over any map.
    pub const OUTLINE: f32 = 2.0;
    /// Persistent Inspect-state frame: two amber edge pixels and a one-pixel ink keyline inside.
    /// Three pixels total is conspicuous on 240×320 without materially shrinking the map.
    pub const FRAME_AMBER_W: i32 = 2;
    pub const FRAME_INK_W: i32 = 1;
    /// Native-panel corner radius. This matches the simulator's 10 px display mask and leaves the
    /// same gentle curve on the physical rounded-corner panel.
    pub const FRAME_RADIUS: u32 = 10;
    /// Zoom's bare plus/minus strokes use the same amber-with-ink-halo treatment as the chevrons.
    pub const ZOOM_GLYPH_HALF: f32 = 7.0;
    pub const ZOOM_GLYPH_HW: f32 = 2.5;
    /// Back-to-you marker — a simple filled triangle in the rider's marker colour (so it
    /// reads as "you" and stays distinct from the hollow amber chevrons). Its half-height
    /// / half-width, inset from the edge, and how far off-screen the rider must be first.
    pub const BACK_H: f32 = 8.0;
    pub const BACK_W: f32 = 7.0;
    pub const BACK_MARGIN: f32 = 14.0;
    pub const OFFSCREEN_MARGIN: f32 = 6.0;
}

#[inline]
fn pt(x: f32, y: f32) -> Point {
    Point::new(round_coord(x), round_coord(y))
}

/// A filled, ink-outlined triangle pointing along `(ux, uy)` — the solid back-to-you marker.
/// `h`/`w` are the half-height and base half-width; the outline is the same triangle grown by
/// [`hud::OUTLINE`], drawn first.
fn outlined_arrow(
    cv: &mut impl Surface,
    center: (f32, f32),
    dir: (f32, f32),
    size: (f32, f32),
    fill: u16,
    outline: u16,
) {
    let (cx, cy) = center;
    let (ux, uy) = dir;
    let (h, w) = size;
    let (perpx, perpy) = (-uy, ux);
    let arrow = |hh: f32, ww: f32| {
        let (bx, by) = (cx - ux * hh, cy - uy * hh); // base centre, opposite the tip
        (pt(cx + ux * hh, cy + uy * hh), pt(bx + perpx * ww, by + perpy * ww), pt(bx - perpx * ww, by - perpy * ww))
    };
    let (ot, obl, obr) = arrow(h + hud::OUTLINE, w + hud::OUTLINE);
    cv.triangle(ot, obl, obr, outline);
    let (t, bl, br) = arrow(h, w);
    cv.triangle(t, bl, br, fill);
}

/// Draw the Inspect HUD over the already-rendered map: a persistent amber frame, the active
/// action's self-explanatory edge cues, and (once the rider is off-screen) a back-to-you marker.
/// Route mode needs no label or false screen-space arrow: frame + movement along the visible route
/// is the feedback. The viewport carries the frozen rotation used by the off-screen test.
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

    // 1) Persistent Inspect frame. Unlike a Route-only label it communicates the important state —
    //    this camera is detached — in Route, Free, and Zoom alike.
    draw_inspect_frame(cv, w as i32, h as i32);

    // 2) Back-to-you marker first, so the chevrons render over it where they overlap. A filled
    //    triangle at the rider's bearing edge crossing.
    if let Some((bx, by, bux, buy)) = user_fix.and_then(|fix| back_to_you(w, h, vp, fix)) {
        outlined_arrow(cv, (bx, by), (bux, buy), (BACK_H, BACK_W), marker, INK);
    }

    // 3) Up/Down cues. Free movement gets directional edge chevrons; Zoom gets unambiguous +/−
    //    cues. Route movement deliberately gets no false screen-space arrow or explanatory chrome:
    //    moving along the visible route is already direct feedback.
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
/// and a thin ink keyline keeps them legible over orange roads and pale map fills. The input
/// plane's hold bulge renders above this, so charging feedback still wins temporarily without
/// erasing the persistent mode cue.
fn draw_inspect_frame(cv: &mut impl Surface, w: i32, h: i32) {
    use super::palette::*;
    use hud::*;
    let total_w = FRAME_AMBER_W + FRAME_INK_W;
    if w <= 2 * total_w || h <= 2 * total_w {
        return;
    }

    // Reducing the radius with each inset keeps the three strokes concentric instead of making
    // the inner corners progressively squarer. At 240x320 the outer 10 px curve is the same curve
    // used by the simulator's display texture and the intended device glass.
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

/// A bare +/- glyph with the same round-ended amber stroke and ink halo as the pan chevrons. The
/// map remains visible around it; there is no white button disc competing with route furniture.
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

/// Where to put the back-to-you marker for an off-screen rider — `(x, y, ux, uy)`, the
/// edge crossing of their bearing plus the unit direction toward them — or `None` while
/// the marker is on-screen (it's already drawn).
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
    // alpha-max-plus-beta-min: a cheap |v| (no sqrt/libm) to normalize the direction —
    // ~4% off, invisible at this size.
    let (adx, ady) = (dx.abs(), dy.abs());
    let mag = adx.max(ady) + 0.41 * adx.min(ady);
    if mag < 1.0 {
        return None;
    }
    let (ux, uy) = (dx / mag, dy / mag);
    // Clamp the bearing to the inset screen rectangle: the nearer of the two border
    // crossings (vertical vs horizontal) is where the marker sits.
    let (hw, hh) = (w / 2.0 - BACK_MARGIN, h / 2.0 - BACK_MARGIN);
    let tx = if adx > 0.01 { hw / adx } else { f32::MAX };
    let ty = if ady > 0.01 { hh / ady } else { f32::MAX };
    let t = tx.min(ty);
    Some((w / 2.0 + dx * t, h / 2.0 + dy * t, ux, uy))
}

/// Draw one active-axis chevron — an open, round-capped "Λ" caret pointing along `dir`, with an
/// even ink halo. Two arm quads at half-width `hw` + round caps/join. Both passes share the
/// centreline, so the halo stays uniform — unlike growing a filled polygon, which warps the arm angle.
fn chevron(cv: &mut impl Surface, center: (f32, f32), dir: (f32, f32), fill: u16, outline: u16) {
    use hud::*;
    let (cx, cy) = center;
    let (ux, uy) = dir;
    let (px, py) = (-uy, ux); // perpendicular = arm spread
    let tip = (cx + ux * CHEV_REACH, cy + uy * CHEV_REACH);
    let lb = (cx - ux * CHEV_BACK - px * CHEV_SPREAD, cy - uy * CHEV_BACK - py * CHEV_SPREAD);
    let rb = (cx - ux * CHEV_BACK + px * CHEV_SPREAD, cy - uy * CHEV_BACK + py * CHEV_SPREAD);
    // Ink halo first (wider), fill on top — same centreline, so the halo is even all round.
    for (hw, color) in [(CHEV_HW + OUTLINE, outline), (CHEV_HW, fill)] {
        rounded_arm(cv, lb, tip, hw, color);
        rounded_arm(cv, tip, rb, hw, color);
        // The shared tip needs its own cap after the two strokes overlap, keeping the join as round
        // and even as the standalone +/- glyphs' endpoints.
        let r = round_coord(hw - 0.5).max(1) as u32;
        cv.disc(pt(tip.0, tip.1), r, color);
    }
}

/// Stroke `a`→`b` with round caps. Shared by the directional chevrons and zoom glyphs so both
/// affordances have exactly the same visual weight on the RGB222 panel.
fn rounded_arm(cv: &mut impl Surface, a: (f32, f32), b: (f32, f32), hw: f32, color: u16) {
    arm(cv, a, b, hw, color);
    // `disc(c, r)` spans diameter `2r+1` (true radius `r+0.5`), so pass `hw-0.5` to make the
    // round cap exactly `hw` wide — matching the arm, not bulging half a pixel past it.
    let r = round_coord(hw - 0.5).max(1) as u32;
    cv.disc(pt(a.0, a.1), r, color);
    cv.disc(pt(b.0, b.1), r, color);
}

/// Stroke segment `a`→`b` as a filled quad (two triangles) of half-width `hw`. The unit
/// normal uses the alpha-max-plus-beta-min |v| approximation (no sqrt/libm; ~4% off,
/// invisible here).
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
    use crate::screen::test_ctx;
    use crate::screen::{Screen, Transition};
    use crate::Settings;

    fn run(act: &mut Activity, g: Gesture) -> Transition {
        let mut st = crate::AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let mut cx = test_ctx(&mut st, act, &mut settings);
        MapScreen::new().handle(g, &mut cx)
    }

    /// The **browse map** (not tracking): `back` pops back to the Menu — there's no Statistics
    /// sibling without a ride.
    #[test]
    fn browse_map_back_pops() {
        let mut act = Activity::new(Mode::Idle); // no session → browse map
        assert!(matches!(run(&mut act, Gesture::Back), Transition::Pop));
    }

    /// The browse map's `press` opens the small start card instead of the Paused page.
    #[test]
    fn browse_map_press_opens_the_start_card() {
        let mut act = Activity::new(Mode::Idle);
        assert!(matches!(run(&mut act, Gesture::Press), Transition::Push(Screen::RideStart(_))));
        assert_eq!(act.mode, Mode::Idle, "opening the card doesn't touch the mode");
    }

    /// The **riding map** (tracking): `back` swaps to the Statistics sibling, `press` pauses into
    /// Ride control — the mid-ride bindings, unchanged.
    #[test]
    fn riding_map_keeps_the_sibling_and_pause_bindings() {
        let mut act = Activity::new(Mode::Riding);
        act.start_session();
        assert!(matches!(run(&mut act, Gesture::Back), Transition::Replace(Screen::Statistics(_))));
        let mut act = Activity::new(Mode::Riding);
        act.start_session();
        assert!(matches!(run(&mut act, Gesture::Press), Transition::Push(Screen::RideControl(_))));
    }

    /// The chosen bar is always the largest 1/2/5×10ⁿ that fits the target width, so across the whole
    /// zoom range the realised pixel width stays in a sane band: never wider than the target, and wide
    /// enough to read (the 1/2/5 steps keep it above ~⅓ of the max). Concrete `1/2/5` values are pinned
    /// by [`scale_bar_labels_are_correct`].
    #[test]
    fn scale_bar_fits_the_target_across_the_zoom_range() {
        // A sweep from riding-close (0.5 m/px) to overview (400 m/px), both unit systems.
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

    /// Concrete labels at representative zooms — the metric metres↔km and imperial feet↔miles
    /// cross-overs, so a regression in the rounding/format shows up as a wrong string.
    #[test]
    fn scale_bar_labels_are_correct() {
        assert_eq!(scale_bar_choice(1.0, Units::Metric).unwrap().1.as_str(), "50m");
        assert_eq!(scale_bar_choice(10.0, Units::Metric).unwrap().1.as_str(), "500m");
        assert_eq!(scale_bar_choice(50.0, Units::Metric).unwrap().1.as_str(), "2km");
        assert_eq!(scale_bar_choice(200.0, Units::Metric).unwrap().1.as_str(), "10km");
        // Imperial: sub-mile feet, then whole 1/2/5 miles past 5280 ft — never a feet-rounded
        // fraction like "1.8mi".
        assert_eq!(scale_bar_choice(1.0, Units::Imperial).unwrap().1.as_str(), "200ft");
        assert_eq!(scale_bar_choice(10.0, Units::Imperial).unwrap().1.as_str(), "2000ft");
        assert_eq!(scale_bar_choice(50.0, Units::Imperial).unwrap().1.as_str(), "2mi");
        assert_eq!(scale_bar_choice(200.0, Units::Imperial).unwrap().1.as_str(), "10mi");
    }

    /// A degenerate camera (non-finite or non-positive mpp) yields no bar, never a bogus one.
    #[test]
    fn scale_bar_rejects_degenerate_zoom() {
        assert!(scale_bar_choice(f32::NAN, Units::Metric).is_none());
        assert!(scale_bar_choice(f32::INFINITY, Units::Metric).is_none());
        assert!(scale_bar_choice(0.0, Units::Metric).is_none());
        assert!(scale_bar_choice(-1.0, Units::Metric).is_none());
    }

    /// The diamond helper: an on-panel centre yields the four rhombus vertices ±r about it; a centre
    /// straddling an edge (within the half-diagonal margin) still draws; one wholly past the margin
    /// culls to `None`.
    #[test]
    fn waypoint_diamond_vertices_and_cull() {
        let r = WAYPOINT_DIAMOND_R;
        // On-panel: the four vertices sit ±r about the centre.
        let v = waypoint_diamond(100, 80, 240, 240).expect("on-panel centre draws");
        assert_eq!(
            v,
            (Point::new(100, 80 - r), Point::new(100, 80 + r), Point::new(100 - r, 80), Point::new(100 + r, 80))
        );
        // Straddling an edge (centre exactly on the half-diagonal margin): still drawn.
        assert!(waypoint_diamond(-r, 120, 240, 240).is_some(), "just off the left edge still draws");
        assert!(waypoint_diamond(120, 240 + r, 240, 240).is_some(), "just past the bottom still draws");
        // Wholly off-panel beyond the margin: culled.
        assert!(waypoint_diamond(-r - 1, 120, 240, 240).is_none(), "past the left margin culls");
        assert!(waypoint_diamond(240 + r + 1, 120, 240, 240).is_none(), "past the right margin culls");
        assert!(waypoint_diamond(120, -r - 1, 240, 240).is_none(), "above the top margin culls");
    }

    fn dt(hour: u8, minute: u8) -> DateTime {
        DateTime { year: 2025, month: 6, day: 29, hour, minute }
    }

    /// The clock overlay's minute tick: with the pill visible a rollover self-dirties **only** the
    /// pill region and arms the next-minute wake; hidden (setting off or panning) it claims nothing
    /// and arms no wake.
    #[test]
    fn clock_tick_is_region_scoped_and_gated() {
        let w = 240;
        let mut scr = MapScreen::new();
        // `tracking = true` throughout so the browse hint retires to `Done` on the first poll and
        // never contends for the wake — this case pins the clock overlay alone.
        // First observation just initialises the baseline (no change), and arms the minute wake.
        let t0 = scr.tick_timers(0, dt(14, 40), 20_000, w, false, true, true);
        assert!(!t0.changed);
        assert_eq!(t0.next_wake_ms, Some(20_000));
        // A minute rollover fires, region-clipped to the pill (never a full-frame None).
        let t1 = scr.tick_timers(1_000, dt(14, 41), 60_000, w, false, true, true);
        assert!(t1.changed);
        assert_eq!(t1.region, Some(clock_region(w)));
        assert!(t1.region.unwrap().size.width < w as u32, "the pill region is a small band, not the whole width");
        // Hidden by the setting: no change, no wake, even across a rollover.
        let off = scr.tick_timers(2_000, dt(14, 42), 60_000, w, false, false, true);
        assert_eq!(off, ScreenTick::idle());
        // Hidden by pan: same — the pan chevron owns the slot.
        let panned = scr.tick_timers(3_000, dt(14, 43), 60_000, w, true, true, true);
        assert_eq!(panned, ScreenTick::idle());
    }

    /// The browse-map start hint (T6 #684): the first poll of a **browse** map (not tracking) arms it
    /// — the chip is up and a wake is asked for `HINT_MS` out — and it fires exactly one clearing
    /// repaint at expiry, then stays down. The clock is off here (`map_clock = false`) to isolate the
    /// hint's own wake.
    #[test]
    fn browse_hint_arms_on_entry_and_expires_once() {
        let mut scr = MapScreen::new();
        // Entry: armed, chip up, wake at HINT_MS — but nothing "changed" (it was drawn on entry).
        let t0 = scr.tick_timers(0, dt(14, 40), 60_000, 240, false, false, false);
        assert!(!t0.changed);
        assert_eq!(t0.next_wake_ms, Some(HINT_MS));
        assert!(scr.hint.chip_up(), "the chip is up on entry");
        // Still up just before the deadline, with a shrinking residual wake.
        let mid = scr.tick_timers(HINT_MS - 1, dt(14, 40), 60_000, 240, false, false, false);
        assert!(!mid.changed);
        assert_eq!(mid.next_wake_ms, Some(1));
        assert!(scr.hint.chip_up());
        // At the deadline: one clearing repaint (full-frame — the chip band + scale-bar step), then down.
        let expire = scr.tick_timers(HINT_MS, dt(14, 40), 60_000, 240, false, false, false);
        assert!(expire.changed);
        assert_eq!(expire.region, None, "the expiry is a full-frame change, not a clock-region tick");
        assert_eq!(expire.next_wake_ms, None);
        assert!(!scr.hint.chip_up(), "the chip is down once expired");
        // And it does not re-fire on a later poll (the same instance stays retired).
        let after = scr.tick_timers(HINT_MS + 5_000, dt(14, 40), 60_000, 240, false, false, false);
        assert!(!after.changed);
        assert!(!scr.hint.chip_up());
    }

    /// A **riding** map (tracking) never shows the hint: the first poll retires it straight to `Done`,
    /// so the chip is down from the start and arms no hint wake.
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

    /// The waypoint chip's pure visibility helper: shown only when not panning, the warning chip is
    /// down, a next waypoint exists and is in range, and the mode allows — with the approach radius
    /// honoured to the exact metre.
    #[test]
    fn waypoint_chip_visibility_rules() {
        let wpts = [wp(0, "Brunnen"), wp(1700, "Pass Summit")];
        let next = Some(1); // the next waypoint is Pass Summit at 1700 m
        let approach = |p| waypoint_chip(WaypointMode::Approach, false, false, false, next, &wpts, p);
        // Approach: hidden beyond the radius, shown from exactly 500 m out (not 501 m).
        assert_eq!(approach(1000), None, "700 m out: still hidden");
        assert_eq!(approach(1199), None, "501 m out: still hidden");
        assert_eq!(approach(1200), Some((1, 500)), "exactly 500 m out: shown, counting 500");
        assert_eq!(approach(1201), Some((1, 499)), "inside the radius: shown, counting down");
        // Always: shown at any distance ahead; Off: never.
        assert_eq!(
            waypoint_chip(WaypointMode::Always, false, false, false, next, &wpts, 0),
            Some((1, 1700)),
            "Always shows the far waypoint too"
        );
        assert_eq!(waypoint_chip(WaypointMode::Off, false, false, false, next, &wpts, 1200), None, "Off never shows");
        // The three suppressors, each over an Always frame that would otherwise show.
        assert_eq!(waypoint_chip(WaypointMode::Always, true, false, false, next, &wpts, 0), None, "panning hides it");
        assert_eq!(waypoint_chip(WaypointMode::Always, false, true, false, next, &wpts, 0), None, "no-fix hides it");
        assert_eq!(waypoint_chip(WaypointMode::Always, false, false, true, next, &wpts, 0), None, "off-route hides it");
        // No next waypoint (route done / none loaded), and a stale index past the table, both cull safely.
        assert_eq!(waypoint_chip(WaypointMode::Always, false, false, false, None, &wpts, 0), None, "no next waypoint");
        assert_eq!(
            waypoint_chip(WaypointMode::Always, false, false, false, Some(9), &wpts, 0),
            None,
            "stale index culls"
        );
    }

    /// The pass-linger: for the ~100 m the resident index still points at a just-passed waypoint,
    /// `dist_to_go` clamps to 0 — the "you are here" readout — so the chip shows `0m`, never a
    /// wrapped/negative distance.
    #[test]
    fn waypoint_chip_lingers_at_zero_past_the_waypoint() {
        let wpts = [wp(1700, "Pass Summit")];
        // 50 m past the waypoint the index (still 0) lingers; the distance clamps to 0.
        let got = waypoint_chip(WaypointMode::Approach, false, false, false, Some(0), &wpts, 1750);
        assert_eq!(got, Some((0, 0)), "50 m past: visible, distance clamped to 0");
        assert_eq!(crate::stat_fields::fmt_dist_short(0, Units::Metric).as_str(), "0m", "…rendering as 0m");
    }

    /// The chip name fits its pixel budget: short names pass through verbatim, long ones are cut to
    /// leading chars + an ASCII ellipsis (the device font is ASCII-only) that stays within budget.
    #[test]
    fn waypoint_chip_name_truncation_fits_the_budget() {
        let cw = Font::Body.char_width() as i32;
        let mut buf = heapless::String::<28>::new();
        assert_eq!(fit_name("Brunnen", 100 * cw, &mut buf), "Brunnen", "a name within budget is verbatim");
        let mut buf = heapless::String::<28>::new();
        let fitted = fit_name("Pass Summit Overlook", 10 * cw, &mut buf);
        assert_eq!(fitted, "Pass Sum..", "8 leading chars + two-dot ellipsis fill the 10-cell budget");
        assert!((text_width(fitted, Font::Body) as i32) <= 10 * cw, "and it stays within budget");
    }

    /// The T10 acceptance case: the canonical `◆ Pass Summit  299m` chip on the 240 px panel gives
    /// the name its whole remaining width, so "Pass Summit" reads in full — never a truncated
    /// `Pass ..`. Recomputes the name budget the way [`draw_waypoint_chip`] does.
    #[test]
    fn pass_summit_fits_the_approach_chip_at_240px() {
        let w = 240;
        let font = Font::Body;
        let diamond_w = 2 * WPT_CHIP_DIAMOND_R + 1;
        let dist_w = text_width("299m", font) as i32;
        let fixed_w = 2 * WPT_CHIP_PAD_X + diamond_w + WPT_CHIP_GAP_D + WPT_CHIP_GAP_N + dist_w;
        let name_budget = (w - 2 * WPT_CHIP_INSET_X) - fixed_w;
        let mut buf = heapless::String::<28>::new();
        assert_eq!(fit_name("Pass Summit", name_budget, &mut buf), "Pass Summit", "the full name fits, no ellipsis");
    }
}
