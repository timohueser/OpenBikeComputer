//! The Map screen — the Riding view. The camera lives in [`AppState`](crate::AppState) (shared with
//! the host's pan/zoom); the screen itself holds only a [`MinuteTicker`] for the clock overlay. `draw`
//! renders the base map plus the route, travel chevrons, breadcrumb, user marker, and the map chrome:
//! floating top-centre clock digits, a bottom-centre one-slot warning chip, a bottom-left scale bar
//! (stepping up above the chip while one is up), a low-battery cue in the top-left corner, and the
//! pan HUD.
//!
//! Bindings: `turn` = zoom, `press` = pause → Ride control, `back` = the sibling Statistics view,
//! `back-hold` = Menu, `hold` = enter Pan mode.

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

use crate::app::{Pan, PanAxis};
use crate::hal::Fix;
use crate::input::Gesture;
use crate::settings::{DateTime, Units};
use crate::wall_clock::MinuteTicker;

use super::{Ctx, Render, Screen, ScreenTick, StatisticsScreen, Transition};

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

/// The live map / Follow view. The camera is the shared [`AppState`](crate::AppState); the only
/// screen-local state is the clock overlay's [`MinuteTicker`], which fires a region-clipped repaint
/// of the clock digits once each minute the wall clock rolls over.
#[derive(Debug, Default)]
pub struct MapScreen {
    /// Fires a repaint of the clock digits each minute the wall clock rolls over (see
    /// [`tick_timers`](MapScreen::tick_timers)) so `HH:MM` advances without a full map redraw.
    ticker: MinuteTicker,
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
    pub fn tick_timers(
        &mut self,
        now: DateTime,
        ms_to_next_minute: u32,
        w: i32,
        pan_active: bool,
        map_clock: bool,
    ) -> ScreenTick {
        if !map_clock || pan_active || w == 0 {
            // Hidden (or no frame drawn yet): observe the minute so a later show doesn't fire a
            // stale rollover, but claim nothing and arm no wake.
            let _ = self.ticker.changed(now);
            return ScreenTick::idle();
        }
        ScreenTick {
            changed: self.ticker.changed(now),
            next_wake_ms: Some(ms_to_next_minute),
            region: Some(clock_region(w)),
        }
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

    pub fn draw<D, F>(&self, cv: &mut Canvas<D, F>, rx: &mut Render)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        // The Map is the only screen that reads the `Reader`; `None` is unreachable in practice
        // (the host only draws the map with it) — draw nothing rather than fault.
        let Some(reader) = rx.reader else { return };
        let vp = rx.state.viewport(rx.w as f32, rx.h as f32);
        let bg565 = reader.backdrop_style().map_or(DEFAULT_BG_RGB565, |s| s.color);
        // The base map, route, breadcrumb and marker render through the raw target + colour policy —
        // the one consumer of the Canvas escape hatch (everything else draws via `Surface`).
        let (target, color_fn) = cv.split();
        let bg = color_fn(bg565);
        // `render_timed` fills the per-stage timings from `rx.clock`; with a `NoopClock` it's `render`.
        let mut stats = rx.renderer.render_timed(target, reader, &vp, bg, color_fn, rx.clock);

        // Chevrons appear once zoomed past `CHEVRON_MAX_MPP`, anchored to the rider's matched
        // distance (`progress_m`). Gated on the viewport scale, decoupled from the LOD pyramid.
        let arrows_at = (vp.meters_per_pixel() <= CHEVRON_MAX_MPP).then_some(rx.activity.progress_m);

        // The planned route, stroked in magenta under the breadcrumb + marker — handed to the
        // renderer through the `RouteOverlaySource` seam (`RouteOverlay` adapts the reader).
        if let Some(route) = rx.route {
            let (route_chunks, route_points, route_points_drawn) = rx.renderer.draw_route(
                target,
                &vp,
                &crate::route::RouteOverlay(route),
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

        rx.stats = stats;

        // The remaining chrome draws in the palette vocabulary, back through the canvas.
        let panning = rx.state.pan.is_some();

        // Low-battery cue (top-left corner): a small warning-red battery glyph only when the charge
        // has dropped below LOW_BATTERY_PCT — nothing above it, so there's no permanent map battery
        // indicator. Shown in pan mode too (the top-right corner belongs to the pan compass rose).
        if rx.state.battery_pct < LOW_BATTERY_PCT {
            draw_low_battery(cv);
        }

        // Clock (top-centre): a small floating HH:MM — bare ink digits with a parchment halo, no
        // pill, so it informs without drawing the eye. Shown when the setting is on. Hidden while
        // panning — the pan HUD's top chevron / compass own the top edge — so it never fights the
        // chevron; `tick_timers` mirrors that gate when arming the minute wake.
        if rx.settings.map_clock && !panning {
            draw_clock(cv, rx.w, rx.now);
        }

        // Bottom-centre one-slot status chip, shown only when there's something to say. "No GPS
        // Fix" takes priority over off-route: with no fix the match is stale, so cross-track
        // distance is meaningless. Suppressed while panning — the pan HUD's bottom chevron owns the
        // bottom-centre slot (they'd collide), and panning is deliberate map inspection anyway; the
        // chip returns the moment pan exits.
        let chip_up = !panning && (rx.no_fix || rx.activity.off_route);
        if chip_up {
            if rx.no_fix {
                draw_status_chip(cv, rx.w, rx.h, "No GPS Fix");
            } else {
                let mut s: heapless::String<20> = heapless::String::new();
                super::write_off_route(&mut s, "off route ", rx.activity.dist_to_route_m, rx.settings.units);
                draw_status_chip(cv, rx.w, rx.h, &s);
            }
        }

        // Scale bar (bottom-left): the largest round distance that fits the target on-screen width
        // at the current zoom, in the units setting's system. Right in the corner — except while
        // the warning chip is up, when it steps to just above the chip band so a wide chip
        // ("off route 153km") never runs under it. Visible in pan mode too (where it's most
        // useful) — the pan HUD's bottom chevron is centred, well clear of the corner.
        if rx.settings.map_scale_bar {
            draw_scale_bar(cv, rx.h, chip_up, vp.meters_per_pixel(), rx.settings.units);
        }

        // Pan-mode HUD. Drawn last so it sits over the map + marker, and only while panning.
        if let Some(pan) = rx.state.pan {
            draw_pan_hud(cv, (rx.w as f32, rx.h as f32), pan, rx.state.user_fix, marker565, &vp);
        }
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

/// The status chip's band height and its inset from the bottom edge (above the panel frame, below
/// where the pan bottom chevron would draw — the two never coexist; the chip is pan-suppressed).
const CHIP_H: i32 = 36;
const CHIP_MARGIN: i32 = 10;

// ---- Clock (top-centre) ---------------------------------------------------

/// Top inset of the floating `HH:MM` digits.
const CLOCK_TOP: i32 = 8;

/// The rectangle the top-centre clock digits occupy, in panel pixels — the dirty region
/// [`tick_timers`](MapScreen::tick_timers) reports so the host clips the minute repaint to just the
/// digits instead of re-rendering the whole map plane. Sized for a fixed 5-glyph `HH:MM` in
/// [`Font::Label`] — constant, so the region doesn't shift as the digits change (`11` vs `22`) —
/// with two pixels of margin all round covering the halo strokes.
pub fn clock_region(w: i32) -> Rectangle {
    let tw = text_width("00:00", Font::Label) as i32;
    let th = Font::Label.line_height() as i32;
    Rectangle::new(Point::new((w - tw) / 2 - 2, CLOCK_TOP - 2), Size::new(tw as u32 + 4, th as u32 + 4))
}

/// Draw the small top-centre `HH:MM` clock: bare ink digits floating on the map — no pill, just the
/// scale-bar label's parchment halo so they stay readable over dark terrain. Deliberately *muted*
/// (the warning-orange of [`draw_status_chip`] stays reserved for alerts). "Small and simple, just
/// readable."
fn draw_clock(cv: &mut impl Surface, w: i32, now: DateTime) {
    use super::palette::*;
    let mut s: heapless::String<8> = heapless::String::new();
    let _ = write!(s, "{:02}:{:02}", now.hour, now.minute);
    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        cv.text(&s, Point::new(w / 2 + dx, CLOCK_TOP + dy), Font::Label, TextAlign::Center, PARCHMENT);
    }
    cv.text(&s, Point::new(w / 2, CLOCK_TOP), Font::Label, TextAlign::Center, INK);
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
/// band (its height + inset + a gap) while the warning chip is up.
const SCALE_MARGIN_Y: i32 = 12;
const SCALE_MARGIN_Y_CHIP: i32 = CHIP_H + CHIP_MARGIN + 12;
const SCALE_TICK_H: i32 = 5;

/// Draw the scale bar at the bottom-left: a horizontal ink line with end ticks and a length label,
/// haloed in parchment so it reads over terrain. The distance is the largest 1/2/5 × 10ⁿ that fits
/// [`SCALE_TARGET_MIN_PX`]..[`SCALE_TARGET_MAX_PX`] at the current `mpp` (metres per pixel), in the
/// `units` system.
fn draw_scale_bar(cv: &mut impl Surface, h: i32, chip_up: bool, mpp: f32, units: Units) {
    use super::palette::*;
    let Some((bar_px, label)) = scale_bar_choice(mpp, units) else {
        return; // a degenerate zoom (non-finite mpp) — draw nothing rather than a bogus bar
    };
    let x0 = SCALE_MARGIN_X;
    let x1 = x0 + bar_px;
    let y = h - if chip_up { SCALE_MARGIN_Y_CHIP } else { SCALE_MARGIN_Y };
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

/// Draw the pan-mode HUD over the already-rendered map: an open chevron on each of the active
/// axis's edges, the frozen-orientation compass, and (only once the rider is off-screen) a back-to-
/// you marker. `vp` carries the frozen pan rotation, so the compass needle and off-screen test
/// agree with what's drawn.
fn draw_pan_hud(cv: &mut impl Surface, size: (f32, f32), pan: Pan, user_fix: Option<Fix>, marker: u16, vp: &Viewport) {
    use super::palette::*;
    use hud::*;
    let (w, h) = size;

    // 1) Back-to-you marker first, so the chevrons render over it where they overlap. A filled
    //    triangle at the rider's bearing edge crossing.
    if let Some((bx, by, bux, buy)) = user_fix.and_then(|fix| back_to_you(w, h, vp, fix)) {
        outlined_arrow(cv, (bx, by), (bux, buy), (BACK_H, BACK_W), marker, INK);
    }

    // 2) Active-axis chevrons: one hollow caret on each of the axis's two edges.
    let chevs: [((f32, f32), (f32, f32)); 2] = match pan.axis {
        PanAxis::Vertical => [((w / 2.0, CHEV_INSET), (0.0, -1.0)), ((w / 2.0, h - CHEV_INSET), (0.0, 1.0))],
        PanAxis::Horizontal => [((CHEV_INSET, h / 2.0), (-1.0, 0.0)), ((w - CHEV_INSET, h / 2.0), (1.0, 0.0))],
    };
    for (center, dir) in chevs {
        chevron(cv, center, dir, AMBER, INK);
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
        arm(cv, lb, tip, hw, color);
        arm(cv, tip, rb, hw, color);
        // `disc(c, r)` spans diameter `2r+1` (true radius `r+0.5`), so pass `hw-0.5` to make
        // the round cap exactly `hw` wide — matching the arm, not bulging half a pixel past it.
        let r = round_coord(hw - 0.5).max(1) as u32;
        cv.disc(pt(lb.0, lb.1), r, color);
        cv.disc(pt(tip.0, tip.1), r, color);
        cv.disc(pt(rb.0, rb.1), r, color);
    }
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
        // First observation just initialises the baseline (no change), and arms the minute wake.
        let t0 = scr.tick_timers(dt(14, 40), 20_000, w, false, true);
        assert!(!t0.changed);
        assert_eq!(t0.next_wake_ms, Some(20_000));
        // A minute rollover fires, region-clipped to the pill (never a full-frame None).
        let t1 = scr.tick_timers(dt(14, 41), 60_000, w, false, true);
        assert!(t1.changed);
        assert_eq!(t1.region, Some(clock_region(w)));
        assert!(t1.region.unwrap().size.width < w as u32, "the pill region is a small band, not the whole width");
        // Hidden by the setting: no change, no wake, even across a rollover.
        let off = scr.tick_timers(dt(14, 42), 60_000, w, false, false);
        assert_eq!(off, ScreenTick::idle());
        // Hidden by pan: same — the pan chevron owns the slot.
        let panned = scr.tick_timers(dt(14, 43), 60_000, w, true, true);
        assert_eq!(panned, ScreenTick::idle());
    }
}
