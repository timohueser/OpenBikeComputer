//! The Map screen — the Riding view. This is the refactor of the old
//! `App::render_frame` map path: it owns no state of its own (the camera lives in
//! [`AppState`](crate::AppState), shared with the host's mouse pan/zoom), and its
//! `draw` is byte-for-byte the previous map + marker render.
//!
//! Bindings (`docs/ui_framework_brief.md` §Screens): `turn` = zoom, `press` =
//! pause → Ride control, `back-hold` = Menu. `hold` (Pan mode) and `back`
//! (Elevation) are reserved until those screens land.

use embedded_graphics::draw_target::DrawTarget;
use obcm_render::RenderStats;

use crate::activity::Mode;
use crate::input::Gesture;

use super::{Ctx, MenuScreen, Render, RideControl, Screen, Transition};

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
            Gesture::Back => Transition::None, // Elevation — later slice
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

        // The user-position marker, resolved through the host color_fn like styles.
        if let Some(fix) = rx.state.user_fix {
            let marker = color_fn(rx.reader.marker_color);
            rx.renderer.draw_marker(target, &vp, fix.lon, fix.lat, fix.course, marker);
        }
        stats
    }
}
