//! The Map screen — the Riding view. This is the refactor of the old
//! `App::render_frame` map path: it owns no state of its own (the camera lives in
//! [`AppState`](crate::AppState), shared with the host's mouse pan/zoom), and its
//! `draw` is byte-for-byte the previous map + marker render.
//!
//! Bindings (`docs/ui_framework_brief.md` §Screens): `turn` = zoom, `press` =
//! pause → Ride control, `back` = the sibling Statistics view, `back-hold` = Menu.
//! `hold` (Pan mode) is reserved until that screen lands.

use core::fmt::Write;

use embedded_graphics::{draw_target::DrawTarget, prelude::Point};
use obcm_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Canvas, RenderStats,
};

use crate::activity::Mode;
use crate::input::Gesture;

use super::{Ctx, MenuScreen, Render, RideControl, Screen, StatisticsScreen, Transition};

/// Zoom multiplier per encoder detent (matches the scroll-wheel feel).
const ZOOM_STEP: f32 = 1.2;
/// Zoom clamps (pixels per microdegree-lat), same spirit as the sim's bounds.
const MIN_ZOOM: f32 = 1e-6;
const MAX_ZOOM: f32 = 1e4;

/// Fallback backdrop when a map carries no backdrop style — mirrors the constant
/// the old `App::render_frame` used, so a backdrop-less map looks identical.
const DEFAULT_BG_RGB565: u16 = 0x2104;

/// Stroke width (px) of the active-route overlay — bold enough to read over the map.
const ROUTE_WEIGHT: u32 = 3;

/// The live map / Follow view. Unit struct — all its state is the shared camera.
#[derive(Debug, Default)]
pub struct MapScreen;

impl MapScreen {
    pub fn new() -> Self {
        MapScreen
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
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
            Gesture::Press => {
                // Pause: tracking stops and the Ride control overlay opens.
                cx.activity.mode = Mode::Paused;
                Transition::Push(Screen::RideControl(RideControl::new()))
            }
            Gesture::Hold => Transition::None, // Pan mode — later slice
            // Swap to the sibling Statistics view (the stack stays one deep); its `back`
            // swaps straight back here.
            Gesture::Back => Transition::Replace(Screen::Statistics(StatisticsScreen::new())),
            Gesture::BackHold => Transition::Push(Screen::Menu(MenuScreen::new())),
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
        let stats = rx.renderer.render(target, rx.reader, &vp, bg, color_fn);

        // The active route, stroked in amber over the map (under the marker).
        if let Some(route) = rx.route {
            rx.renderer.draw_route(target, &vp, route, color_fn(super::palette::AMBER), ROUTE_WEIGHT);
        }

        // The user-position marker, resolved through the host color_fn like styles. It
        // turns warning-red while off-route, so a glance at the map shows the rider has
        // strayed (the active amber route stays drawn — it's the line back).
        if let Some(fix) = rx.state.user_fix {
            let marker565 = if rx.activity.off_route { super::palette::WARNING } else { rx.reader.marker_color };
            rx.renderer.draw_marker(target, &vp, fix.lon, fix.lat, fix.course, color_fn(marker565));
        }

        // Off-route pill: a small parchment chip with the cross-track distance, shown
        // *only* while off-route so the map's steady state stays chrome-free ("map only").
        if rx.activity.off_route {
            draw_off_route_pill(target, rx, color_fn);
        }
        stats
    }
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
    let mut s: heapless::String<20> = heapless::String::new();
    let _ = write!(s, "off route {}m", rx.activity.dist_to_route_m);
    // Bold (Body font) so it's readable at a glance over the map.
    let font = Font::Body;
    let tw = text_width(&s, font) as i32;
    let (pw, ph) = (tw + 24, 26);
    let px = (w - pw) / 2;
    let py = 10;
    cv.round(rect(px, py, pw, ph), 7, PARCHMENT);
    cv.round_outline(rect(px, py, pw, ph), 7, WARNING);
    cv.text(&s, Point::new(w / 2, py + 6), font, TextAlign::Center, WARNING);
}
