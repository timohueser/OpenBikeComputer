//! The Map screen — the Riding view. It owns no state of its own (the camera lives in
//! [`AppState`](crate::AppState), shared with the host's pan/zoom); `draw` renders the base map plus
//! the route, travel chevrons, breadcrumb, user marker, off-route pill, and pan HUD.
//!
//! Bindings: `turn` = zoom, `press` = pause → Ride control, `back` = the sibling Statistics view,
//! `back-hold` = Menu, `hold` = enter Pan mode.

use embedded_graphics::{draw_target::DrawTarget, prelude::Point};
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Canvas, RenderStats, Viewport,
};

use crate::app::{Pan, PanAxis};
use crate::hal::Fix;
use crate::input::Gesture;

use super::{Ctx, Render, Screen, StatisticsScreen, Transition};

/// Zoom multiplier per encoder detent (matches the scroll-wheel feel).
const ZOOM_STEP: f32 = 1.2;
/// Zoom clamps (pixels per microdegree-lat), same spirit as the sim's bounds.
const MIN_ZOOM: f32 = 1e-6;
const MAX_ZOOM: f32 = 1e4;

/// Fallback backdrop when a map carries no backdrop style.
const DEFAULT_BG_RGB565: u16 = 0x2104;

/// Stroke width (px) of the active-route overlay — bold enough to out-weigh the heaviest base road
/// (3 px), and sized so a direction chevron sits *inside* the line at riding zoom.
const ROUTE_WEIGHT: u32 = 11;

/// Colour of the route direction chevrons — white, for contrast over the magenta route line. Drawn
/// only at riding zoom (see [`CHEVRON_MAX_MPP`]).
const ARROW_COLOR: u16 = super::palette::PARCHMENT;

/// Zoom threshold (ground meters per pixel) at/below which the chevrons are drawn — roughly riding
/// scale — fading out on wider overviews. A scale gate, independent of the map's LOD pyramid.
const CHEVRON_MAX_MPP: f32 = 4.0;

/// Stroke width (px) of the breadcrumb — thinner than the route, so the route stays dominant where
/// the two coincide.
const BREADCRUMB_WEIGHT: u32 = 3;

/// The live map / Follow view. Unit struct — all its state is the shared camera.
#[derive(Debug, Default)]
pub struct MapScreen;

impl MapScreen {
    pub fn new() -> Self {
        MapScreen
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        // Pan mode is a sub-mode of the Map: while the shared camera holds a `pan`,
        // the encoder/Back drive panning instead of the Follow bindings below.
        if cx.state.pan.is_some() {
            return handle_pan(g, cx);
        }
        match g {
            Gesture::Turn(n) => {
                // Multiply per detent (no_std: no powf).
                let step = if n >= 0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
                let mut z = cx.state.zoom;
                for _ in 0..n.unsigned_abs() {
                    z *= step;
                }
                cx.state.zoom = z.clamp(MIN_ZOOM, MAX_ZOOM);
                Transition::None
            }
            // hold = enter Pan mode: the camera detaches and the pan HUD appears.
            Gesture::Hold => {
                cx.state.enter_pan();
                Transition::None
            }
            // Swap to the sibling Statistics view; its `back` swaps straight back here.
            Gesture::Back => Transition::Replace(Screen::Statistics(StatisticsScreen::new())),
            Gesture::Press | Gesture::BackHold => super::riding_common(g, cx),
        }
    }

    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        // The Map is the only screen that reads the `Reader`; `None` is unreachable in practice
        // (the host only draws the map with it) — draw nothing rather than fault.
        let Some(reader) = rx.reader else { return RenderStats::default() };
        let vp = rx.state.viewport(rx.w, rx.h);
        let bg565 = reader.backdrop_style().map_or(DEFAULT_BG_RGB565, |s| s.color);
        let bg = color_fn(bg565);
        // `render_timed` fills the per-stage timings from `rx.clock`; with a `NoopClock` it's `render`.
        let mut stats = rx.renderer.render_timed(target, reader, &vp, bg, color_fn, rx.clock);

        // Chevrons appear once zoomed past `CHEVRON_MAX_MPP`, anchored to the rider's matched
        // distance (`progress_m`). Gated on the viewport scale, decoupled from the LOD pyramid.
        let arrows_at = (vp.meters_per_pixel() <= CHEVRON_MAX_MPP).then_some(rx.activity.progress_m);

        // The planned route, stroked in magenta under the breadcrumb + marker.
        if let Some(route) = rx.route {
            let (route_chunks, route_points, route_points_drawn) = rx.renderer.draw_route(
                target,
                &vp,
                route,
                color_fn(super::palette::ROUTE),
                ROUTE_WEIGHT,
                color_fn(ARROW_COLOR),
                arrows_at,
            );
            stats.route_chunks = route_chunks;
            stats.route_points = route_points;
            stats.route_points_drawn = route_points_drawn;
        }

        // The breadcrumb in navy, drawn over the route (and under the marker). One chained stroke
        // (coarse spine → full-res recent tail), so the tiers never double up.
        if !rx.breadcrumb.is_empty() {
            let trail = color_fn(super::palette::BREADCRUMB);
            rx.renderer.stroke_path(target, &vp, rx.breadcrumb.points(), trail, BREADCRUMB_WEIGHT);
        }

        // The "you" colour: warning-red while off-route, else the map's marker colour. Shared by the
        // marker and the pan pin so the off-screen pin matches the on-screen marker.
        let marker565 = if rx.activity.off_route { super::palette::WARNING } else { reader.marker_color };
        if let Some(fix) = rx.state.user_fix {
            rx.renderer.draw_marker(target, &vp, fix.lon, fix.lat, fix.course, color_fn(marker565));
        }

        // Top-center status pill, shown only when there's something to say. "No GPS Fix" takes
        // priority over off-route: with no fix the match is stale, so cross-track distance is
        // meaningless.
        if rx.no_fix {
            draw_no_fix_pill(target, rx, color_fn);
        } else if rx.activity.off_route {
            draw_off_route_pill(target, rx, color_fn);
        }

        // Pan-mode HUD. Drawn last so it sits over the map + marker, and only while panning.
        if let Some(pan) = rx.state.pan {
            draw_pan_hud(target, (rx.w, rx.h), pan, rx.state.user_fix, marker565, &vp, color_fn);
        }
        stats
    }
}

/// Pan-mode gesture bindings, active while [`AppState::pan`](crate::AppState::pan) is `Some`.
/// `turn` pans along the active axis, `press` toggles the axis, `hold` flips north-up ↔ heading-up,
/// `back` recenters on the rider (staying in pan), `back-hold` exits to Follow. This deliberately
/// overrides the global `back-hold` = Menu while panning — exit pan first to reach the Menu.
fn handle_pan(g: Gesture, cx: &mut Ctx) -> Transition {
    match g {
        Gesture::Turn(n) => cx.state.pan_step(n),
        Gesture::Press => cx.state.toggle_pan_axis(),
        Gesture::Hold => cx.state.toggle_pan_orientation(),
        Gesture::Back => cx.state.recenter_on_user(),
        Gesture::BackHold => cx.state.exit_pan(),
    }
    Transition::None
}

/// A compact "No GPS Fix" chip at the top of the map — shown while the device has no current fix,
/// vanishing the moment one lands. Same look + slot as the off-route pill.
fn draw_no_fix_pill<D, F>(target: &mut D, rx: &Render, color_fn: &F)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    use super::palette::*;
    let w = rx.w as i32;
    let mut cv = Canvas::new(target, color_fn);
    let s = "No GPS Fix";
    let font = Font::Body;
    let tw = text_width(s, font) as i32;
    let (pw, ph) = (tw + 28, 36);
    let px = (w - pw) / 2;
    let py = 10;
    cv.round(rect(px, py, pw, ph), 9, PARCHMENT);
    cv.round_outline(rect(px, py, pw, ph), 9, WARNING);
    cv.text(s, Point::new(w / 2, py + 5), font, TextAlign::Center, WARNING);
}

/// A compact "off route NNNm" chip at the top of the map — shown only while off-route, vanishing on
/// rejoin.
fn draw_off_route_pill<D, F>(target: &mut D, rx: &Render, color_fn: &F)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    use super::palette::*;
    let w = rx.w as i32;
    let mut cv = Canvas::new(target, color_fn);
    let mut s: heapless::String<20> = heapless::String::new();
    super::write_off_route(&mut s, "off route ", rx.activity.dist_to_route_m, rx.settings.units);
    let font = Font::Body;
    let tw = text_width(&s, font) as i32;
    let (pw, ph) = (tw + 28, 36);
    let px = (w - pw) / 2;
    let py = 10;
    cv.round(rect(px, py, pw, ph), 9, PARCHMENT);
    cv.round_outline(rect(px, py, pw, ph), 9, WARNING);
    cv.text(&s, Point::new(w / 2, py + 5), font, TextAlign::Center, WARNING);
}

/// Pan-mode HUD geometry — every tunable pixel size in one place. (The camera-travel-per-detent
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
    /// Compass disc radius and its centre inset from the top-right corner, plus the needle
    /// length (kept inside the face) and base half-width.
    pub const COMPASS_R: f32 = 15.0;
    pub const COMPASS_MARGIN: f32 = 19.0;
    pub const NEEDLE_LEN: f32 = 11.0;
    pub const NEEDLE_W: f32 = 4.0;
    /// Back-to-you marker — a simple filled triangle in the rider's marker colour (so it
    /// reads as "you" and stays distinct from the hollow amber chevrons). Its half-height
    /// / half-width, inset from the edge, and how far off-screen the rider must be first.
    pub const BACK_H: f32 = 8.0;
    pub const BACK_W: f32 = 7.0;
    pub const BACK_MARGIN: f32 = 14.0;
    pub const OFFSCREEN_MARGIN: f32 = 6.0;
}

/// Round an f32 pixel coordinate to the nearest device pixel (no_std, no `libm`).
#[inline]
fn ri(v: f32) -> i32 {
    (v + if v >= 0.0 { 0.5 } else { -0.5 }) as i32
}

#[inline]
fn pt(x: f32, y: f32) -> Point {
    Point::new(ri(x), ri(y))
}

/// A filled, ink-outlined triangle pointing along `(ux, uy)` — the solid back-to-you marker.
/// `h`/`w` are the half-height and base half-width; the outline is the same triangle grown by
/// [`hud::OUTLINE`], drawn first.
fn outlined_arrow<D, F>(
    cv: &mut Canvas<D, F>,
    center: (f32, f32),
    dir: (f32, f32),
    size: (f32, f32),
    fill: u16,
    outline: u16,
) where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
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

/// Draw the pan-mode HUD over the already-rendered map: an open chevron on each of the active
/// axis's edges, the frozen-orientation compass, and (only once the rider is off-screen) a back-to-
/// you marker. `vp` carries the frozen pan rotation, so the compass needle and off-screen test
/// agree with what's drawn.
fn draw_pan_hud<D, F>(
    target: &mut D,
    size: (f32, f32),
    pan: Pan,
    user_fix: Option<Fix>,
    marker: u16,
    vp: &Viewport,
    color_fn: &F,
) where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    use super::palette::*;
    use hud::*;
    let (w, h) = size;
    let mut cv = Canvas::new(target, color_fn);

    // 1) Back-to-you marker first, so the chevrons render over it where they overlap. A filled
    //    triangle at the rider's bearing edge crossing.
    if let Some((bx, by, bux, buy)) = user_fix.and_then(|fix| back_to_you(w, h, vp, fix)) {
        outlined_arrow(&mut cv, (bx, by), (bux, buy), (BACK_H, BACK_W), marker, INK);
    }

    // 2) Active-axis chevrons: one hollow caret on each of the axis's two edges.
    let chevs: [((f32, f32), (f32, f32)); 2] = match pan.axis {
        PanAxis::Vertical => [((w / 2.0, CHEV_INSET), (0.0, -1.0)), ((w / 2.0, h - CHEV_INSET), (0.0, 1.0))],
        PanAxis::Horizontal => [((CHEV_INSET, h / 2.0), (-1.0, 0.0)), ((w - CHEV_INSET, h / 2.0), (1.0, 0.0))],
    };
    for (center, dir) in chevs {
        chevron(&mut cv, center, dir, AMBER, INK);
    }

    // 3) Compass (top-right): parchment disc + ink ring, an amber north needle and a wood south
    //    tail. The needle reads the frozen viewport rotation, so it holds still while panning.
    let (ccx, ccy) = (w - COMPASS_MARGIN, COMPASS_MARGIN);
    cv.disc(pt(ccx, ccy), COMPASS_R as u32, INK);
    cv.disc(pt(ccx, ccy), (COMPASS_R - 2.0) as u32, PARCHMENT);
    let (nux, nuy) = vp.north_screen_unit();
    let (perpx, perpy) = (-nuy, nux);
    let base_l = pt(ccx + perpx * NEEDLE_W, ccy + perpy * NEEDLE_W);
    let base_r = pt(ccx - perpx * NEEDLE_W, ccy - perpy * NEEDLE_W);
    cv.triangle(pt(ccx + nux * NEEDLE_LEN, ccy + nuy * NEEDLE_LEN), base_l, base_r, AMBER);
    cv.triangle(pt(ccx - nux * NEEDLE_LEN, ccy - nuy * NEEDLE_LEN), base_l, base_r, WOOD);
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
fn chevron<D, F>(cv: &mut Canvas<D, F>, center: (f32, f32), dir: (f32, f32), fill: u16, outline: u16)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    use hud::*;
    let (cx, cy) = center;
    let (ux, uy) = dir;
    let (px, py) = (-uy, ux); // perpendicular = arm spread
    let tip = (cx + ux * CHEV_REACH, cy + uy * CHEV_REACH);
    let lb = (cx - ux * CHEV_BACK - px * CHEV_SPREAD, cy - uy * CHEV_BACK - py * CHEV_SPREAD);
    let rb = (cx - ux * CHEV_BACK + px * CHEV_SPREAD, cy - uy * CHEV_BACK + py * CHEV_SPREAD);
    // Ink halo first (wider), fill on top — same centreline, so the halo is even all round.
    for (hw, color) in [(CHEV_HW + OUTLINE, outline), (CHEV_HW, fill)] {
        arm(cv, lb, tip, hw, color);
        arm(cv, tip, rb, hw, color);
        // `disc(c, r)` spans diameter `2r+1` (true radius `r+0.5`), so pass `hw-0.5` to make
        // the round cap exactly `hw` wide — matching the arm, not bulging half a pixel past it.
        let r = ri(hw - 0.5).max(1) as u32;
        cv.disc(pt(lb.0, lb.1), r, color);
        cv.disc(pt(tip.0, tip.1), r, color);
        cv.disc(pt(rb.0, rb.1), r, color);
    }
}

/// Stroke segment `a`→`b` as a filled quad (two triangles) of half-width `hw`. The unit
/// normal uses the alpha-max-plus-beta-min |v| approximation (no sqrt/libm; ~4% off,
/// invisible here).
fn arm<D, F>(cv: &mut Canvas<D, F>, a: (f32, f32), b: (f32, f32), hw: f32, color: u16)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let m = dx.abs().max(dy.abs()) + 0.41 * dx.abs().min(dy.abs());
    let s = if m > 0.001 { hw / m } else { 0.0 };
    let (nx, ny) = (-dy * s, dx * s);
    cv.triangle(pt(a.0 + nx, a.1 + ny), pt(b.0 + nx, b.1 + ny), pt(b.0 - nx, b.1 - ny), color);
    cv.triangle(pt(a.0 + nx, a.1 + ny), pt(b.0 - nx, b.1 - ny), pt(a.0 - nx, a.1 - ny), color);
}
