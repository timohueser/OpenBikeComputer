//! The Map screen — the Riding view. It owns no state of its own (the camera lives in
//! [`AppState`](crate::AppState), shared with the host's mouse pan/zoom); `draw` renders
//! the base map plus the route, travel chevrons, breadcrumb, user marker, off-route pill,
//! and pan HUD.
//!
//! Bindings (`docs/ui_framework_brief.md` §Screens): `turn` = zoom, `press` =
//! pause → Ride control, `back` = the sibling Statistics view, `back-hold` = Menu,
//! `hold` = enter Pan mode.

use core::fmt::Write;

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

/// Stroke width (px) of the active-route overlay — bold enough to read over the map and to
/// out-weigh the heaviest base road (motorway/trunk = 3 px), so the route stays the dominant
/// line. Sized so a direction chevron (see [`ARROW_COLOR`]) sits nicely *inside* the line at
/// riding zoom, the Garmin look — sweep it together with the arrow consts in `obc-render`.
const ROUTE_WEIGHT: u32 = 11;

/// Colour of the route direction chevrons — white, for maximum contrast over the magenta
/// route line (lands on `(255,255,255)` on the device-64 panel). Drawn only at riding zoom
/// (see [`CHEVRON_MAX_MPP`] / [`MapScreen::draw`]). The chevron shape + spacing are tuned in
/// `obc-render` (`ARROW_*`).
const ARROW_COLOR: u16 = super::palette::PARCHMENT;

/// Zoom threshold (ground meters per pixel) at/below which the route direction chevrons are
/// drawn — i.e. they appear at roughly riding scale (the riding view opens at ~0.5 m/px) and
/// fade out on wider overviews where they'd just clutter a short on-screen route. A plain
/// scale gate, independent of the map's LOD pyramid: tune this one number to move the cut-off.
const CHEVRON_MAX_MPP: f32 = 4.0;

/// Stroke width (px) of the travelled-path breadcrumb — a touch thinner than the route, so
/// the planned route stays the dominant line where the two coincide.
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
                // Multiply per detent (no_std: no powf) — `n` is a small count.
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
            // Swap to the sibling Statistics view (the stack stays one deep); its `back`
            // swaps straight back here.
            Gesture::Back => Transition::Replace(Screen::Statistics(StatisticsScreen::new())),
            // press = pause → Ride control, back-hold = Menu (shared by both riding views).
            Gesture::Press | Gesture::BackHold => super::riding_common(g, cx),
        }
    }

    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let vp = rx.state.viewport(rx.w, rx.h);
        let bg565 = rx.reader.backdrop_style().map_or(DEFAULT_BG_RGB565, |s| s.color);
        let bg = color_fn(bg565);
        // `render_timed` fills the per-stage map timings (collect/sort/draw) from `rx.clock`; with
        // the host's `NoopClock` it's the same as `render` with the stage fields left at 0.
        let mut stats = rx.renderer.render_timed(target, rx.reader, &vp, bg, color_fn, rx.clock);

        // Direction chevrons ride the route only at riding zoom: the plain stroke shows at every
        // zoom, the chevrons appear once the view is zoomed in past `CHEVRON_MAX_MPP`, anchored to
        // the rider's matched distance along the route (`progress_m`). Gated on the viewport scale
        // directly, so it's decoupled from the map's LOD pyramid.
        let arrows_at = (vp.meters_per_pixel() <= CHEVRON_MAX_MPP).then_some(rx.activity.progress_m);

        // The planned route, stroked in magenta over the map (under the breadcrumb + marker),
        // with white travel-direction chevrons near the rider at riding zoom.
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

        // The travelled-path breadcrumb in navy, drawn *over* the route (and under the marker)
        // so the trail behind reads navy and the route ahead reads magenta. One chained stroke
        // (coarse spine → full-res recent tail), so the tiers never double up. Skipped when
        // nothing is recorded yet (the bounded buffers can never overrun the scratch).
        if !rx.breadcrumb.is_empty() {
            let trail = color_fn(super::palette::BREADCRUMB);
            rx.renderer.stroke_path(target, &vp, rx.breadcrumb.points(), trail, BREADCRUMB_WEIGHT);
        }

        // The "you" colour: warning-red while off-route (so a glance at the map shows the
        // rider has strayed; the route + breadcrumb stay drawn — the line back),
        // else the map's marker colour. Shared by the marker and the pan pin so the
        // off-screen pin matches the on-screen marker.
        let marker565 = if rx.activity.off_route { super::palette::WARNING } else { rx.reader.marker_color };
        if let Some(fix) = rx.state.user_fix {
            rx.renderer.draw_marker(target, &vp, fix.lon, fix.lat, fix.course, color_fn(marker565));
        }

        // Off-route pill: a small parchment chip with the cross-track distance, shown
        // *only* while off-route so the map's steady state stays chrome-free ("map only").
        if rx.activity.off_route {
            draw_off_route_pill(target, rx, color_fn);
        }

        // Pan-mode HUD (axis chevrons + frozen compass + a back-to-you arrow once the
        // rider drifts off-screen). Drawn last so it sits over the map + marker, and
        // only while panning — the map's steady state stays chrome-free.
        if let Some(pan) = rx.state.pan {
            draw_pan_hud(target, (rx.w, rx.h), pan, rx.state.user_fix, marker565, &vp, color_fn);
        }
        stats
    }
}

/// Pan-mode gesture bindings, active while [`AppState::pan`](crate::AppState::pan) is
/// `Some`. `turn` pans the frozen camera along the active axis, `press` toggles the
/// axis, `hold` flips north-up ↔ heading-up, `back` recenters on the rider (staying in
/// pan), and `back-hold` exits back to Follow. Note this deliberately overrides the
/// global `back-hold` = Menu while panning — exit pan first to reach the Menu.
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

/// A compact "off route NNNm" chip centered at the top of the map — appears only while
/// off-route and vanishes on rejoin, keeping the map otherwise free of chrome.
fn draw_off_route_pill<D, F>(target: &mut D, rx: &Render, color_fn: &F)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    use super::palette::*;
    let w = rx.w as i32;
    let mut cv = Canvas::new(target, color_fn);
    // Compact the distance to whole km past 1 km so the pill stays within the panel width
    // at the Body glyph size (a long "...14515m" would otherwise overrun 240 px).
    let d = rx.activity.dist_to_route_m;
    let mut s: heapless::String<20> = heapless::String::new();
    if d >= 1000 {
        let _ = write!(s, "off route {}km", (d + 500) / 1000);
    } else {
        let _ = write!(s, "off route {}m", d);
    }
    // Bold (Body font) so it's readable at a glance over the map.
    let font = Font::Body;
    let tw = text_width(&s, font) as i32;
    let (pw, ph) = (tw + 28, 36);
    let px = (w - pw) / 2;
    let py = 10;
    cv.round(rect(px, py, pw, ph), 9, PARCHMENT);
    cv.round_outline(rect(px, py, pw, ph), 9, WARNING);
    cv.text(&s, Point::new(w / 2, py + 5), font, TextAlign::Center, WARNING);
}

/// Pan-mode HUD geometry — every tunable pixel size in one place, so there are no
/// magic numbers buried in [`draw_pan_hud`]. (The camera-travel-per-detent knob lives
/// with the pan logic as [`crate::app::PAN_STEP_PX`].)
mod hud {
    /// Active-axis chevron — an open, round-capped caret (the arrow look). Its centreline
    /// is a "Λ": tip `REACH` ahead of the centre, back corners `BACK` behind ± `SPREAD`,
    /// stroked at half-width `HW`. One chevron per active edge, inset `INSET` from it.
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

/// Round an f32 pixel coordinate to the nearest device pixel — no_std, no `libm`
/// (these are tiny UI glyphs, so a branch beats pulling in `roundf`).
#[inline]
fn ri(v: f32) -> i32 {
    (v + if v >= 0.0 { 0.5 } else { -0.5 }) as i32
}

#[inline]
fn pt(x: f32, y: f32) -> Point {
    Point::new(ri(x), ri(y))
}

/// A filled, ink-outlined triangle pointing along the unit vector `(ux, uy)` — the
/// solid back-to-you marker (the open chevrons are drawn separately by [`chevron`]).
/// `h`/`w` are the half-height and base half-width; the outline is the same triangle
/// grown by [`hud::OUTLINE`], drawn first.
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

/// Draw the pan-mode HUD over the already-rendered map: a single open chevron on each
/// of the active axis's two edges, the frozen-orientation compass, and (only once the
/// rider is off-screen) a back-to-you marker in the rider's colour. `vp` is the map's
/// viewport — already carrying the frozen pan rotation — so the compass needle and the
/// off-screen test agree with what's drawn.
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

    // 1) Back-to-you marker first, so the chevrons render *over* it where they overlap
    //    (less pretty than hiding a chevron, but less confusing — user's call). A simple
    //    filled triangle in the rider's marker colour, at their bearing's edge crossing.
    if let Some((bx, by, bux, buy)) = user_fix.and_then(|fix| back_to_you(w, h, vp, fix)) {
        outlined_arrow(&mut cv, (bx, by), (bux, buy), (BACK_H, BACK_W), marker, INK);
    }

    // 2) Active-axis chevrons: one outward-pointing hollow caret on each of the axis's
    //    two edges (the open-arrow look — distinct from the solid back-to-you triangle).
    let chevs: [((f32, f32), (f32, f32)); 2] = match pan.axis {
        PanAxis::Vertical => [((w / 2.0, CHEV_INSET), (0.0, -1.0)), ((w / 2.0, h - CHEV_INSET), (0.0, 1.0))],
        PanAxis::Horizontal => [((CHEV_INSET, h / 2.0), (-1.0, 0.0)), ((w - CHEV_INSET, h / 2.0), (1.0, 0.0))],
    };
    for (center, dir) in chevs {
        chevron(&mut cv, center, dir, AMBER, INK);
    }

    // 3) Compass (top-right): parchment disc + ink ring, an amber north needle and a
    //    wood south tail. The needle reads the (frozen) viewport rotation, so it holds
    //    still while panning and visibly turns when `hold` flips N-up/heading-up.
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

/// Draw one active-axis chevron — an open, round-capped caret pointing along `dir`, with
/// an even ink halo. Stroke the "Λ" centreline (the Canvas has no stroke primitive): two
/// arm quads at half-width `hw` + round caps/join (a disc at each of the three vertices).
/// Because both passes share the centreline, the halo stays uniform — unlike growing a
/// filled polygon, which warps the arm angle and gives the ragged border it replaces.
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
