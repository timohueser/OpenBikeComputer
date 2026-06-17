//! [`AppState`] — the device's view state — and [`App`], the shared per-frame
//! driver that both hosts run.

use embedded_graphics::draw_target::DrawTarget;
use obcm::Reader;
use obcm_render::{MapRenderer, RenderStats, Viewport};

use crate::hal::{Fix, LocationSource};

/// Fallback background when a map has no backdrop style (degenerate / empty style
/// table) — a dark grey in RGB565, mapped through the host's color policy like
/// any other style color so it works on true-color and quantized displays alike.
/// Real maps carry a sea/background backdrop, so this is rarely hit.
const DEFAULT_BG_RGB565: u16 = 0x2104; // ≈ (16, 16, 16)

/// How the camera relates to the user's position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    /// The camera tracks the user — every fix recenters the map on it. This is
    /// the device's normal navigation behavior.
    Follow,
    /// The camera is driven manually (the simulator's mouse pan/zoom) and ignores
    /// the user's position; fixes are still recorded for the marker. A host-only
    /// debugging convenience.
    Free,
}

/// The device's view state: where the camera looks, how zoomed in it is, what
/// mode it's in, and the last known user fix.
///
/// This is the shared core the host renders. The host owns the display size and
/// the [`obcm::MapRenderer`]/draw target; each frame it calls [`update`] with the
/// platform's [`LocationSource`], then [`viewport`] to get the camera to render
/// through. The split keeps display dimensions (240×320 on the device, a resized
/// window on the host) out of the shared state.
///
/// [`update`]: AppState::update
/// [`viewport`]: AppState::viewport
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppState {
    /// Camera center longitude in microdegrees (1e-6°).
    pub cam_lon: f64,
    /// Camera center latitude in microdegrees (1e-6°).
    pub cam_lat: f64,
    /// Pixels per microdegree of latitude (the [`Viewport::zoom`] convention).
    pub zoom: f64,
    /// Whether the camera follows the user or is driven manually.
    pub mode: CameraMode,
    /// Map orientation. `true` rotates the projection so the user's course points
    /// to the top of the screen (heading-up / track-up navigation); `false` keeps
    /// north up. Independent of [`mode`](AppState::mode): the camera can follow the
    /// user in either orientation, and the simulator can rotate while mouse-panning.
    pub heading_up: bool,
    /// The most recent fix from the [`LocationSource`], or `None` before the
    /// first one. Drives the heading-up rotation and the (future) user marker.
    pub user_fix: Option<Fix>,
}

impl AppState {
    /// A fresh state centered at `(cam_lon, cam_lat)` microdegrees with the given
    /// `zoom`, in [`Follow`](CameraMode::Follow) mode (the device default) and no
    /// fix yet.
    pub fn new(cam_lon: f64, cam_lat: f64, zoom: f64) -> Self {
        AppState {
            cam_lon,
            cam_lat,
            zoom,
            mode: CameraMode::Follow,
            heading_up: false,
            user_fix: None,
        }
    }

    /// Advance one tick: poll the location source and, in
    /// [`Follow`](CameraMode::Follow) mode, recenter the camera on the new fix.
    /// In [`Free`](CameraMode::Free) mode the fix is still recorded (for the
    /// marker) but the camera is left wherever the host's pan/zoom put it.
    ///
    /// No fix this tick leaves everything untouched, so a momentary GPS dropout
    /// holds the last camera position rather than snapping anywhere.
    pub fn update(&mut self, loc: &mut dyn LocationSource) {
        if let Some(fix) = loc.poll() {
            self.user_fix = Some(fix);
            if self.mode == CameraMode::Follow {
                self.cam_lon = fix.lon as f64;
                self.cam_lat = fix.lat as f64;
            }
        }
    }

    /// Project the current camera into a [`Viewport`] for a `w`×`h` pixel display.
    /// The host supplies its own dimensions, so the same state renders correctly
    /// to the 240×320 device panel and to a resizable simulator window.
    ///
    /// In [`heading_up`](AppState::heading_up) mode the projection is rotated so
    /// the last fix's `course` points to the top of the screen; with no course (or
    /// north-up) it stays north-up.
    pub fn viewport(&self, w: f64, h: f64) -> Viewport {
        let course_rad = if self.heading_up {
            self.user_fix
                .and_then(|f| f.course)
                .map_or(0.0, |deg| (deg as f64).to_radians())
        } else {
            0.0
        };
        Viewport::new_rotated(w, h, self.cam_lon, self.cam_lat, self.zoom, course_rad)
    }
}

/// The whole device application, ready to run a frame.
///
/// This is the single entry point both hosts share: the simulator and the
/// nRF5340 firmware each construct one `App`, then per frame call [`tick`] with
/// their platform's [`LocationSource`] and [`render_frame`] with their display.
/// All the glue between *state* and *pixels* — LOD-driving viewport, backdrop
/// background, the reusable [`MapRenderer`] and its scratch buffers — lives here
/// so neither host reimplements it.
///
/// ```ignore
/// // Both hosts run this same shape:
/// let mut app = App::new(AppState::new(cx, cy, zoom));
/// loop {
///     app.tick(&mut location_source);          // GPS / control panel / GPX
///     app.render_frame(&mut display, &reader,   // SDL-free buffer / LS021B7DD02
///                      w, h, color_policy);
/// }
/// ```
///
/// [`tick`]: App::tick
/// [`render_frame`]: App::render_frame
pub struct App {
    /// The view state — public so the host's UI (control panel, buttons) can read
    /// and adjust mode/zoom/camera directly.
    pub state: AppState,
    /// Reused renderer; clears (not frees) its scratch each frame, so steady-state
    /// rendering does no allocation — important on the MCU.
    renderer: MapRenderer,
}

impl App {
    pub fn new(state: AppState) -> Self {
        App { state, renderer: MapRenderer::new() }
    }

    /// Advance the app one tick from the location source. Buttons
    /// ([`InputSource`](crate::InputSource)) join this signature when input
    /// handling lands.
    pub fn tick(&mut self, loc: &mut dyn LocationSource) {
        self.state.update(loc);
    }

    /// Render the current state into `target`, a `w`×`h` pixel display.
    ///
    /// `color_fn` maps a style's RGB565 to the target's pixel color — the one
    /// genuinely display-specific policy (the simulator picks true-color vs.
    /// device-64 quantization; the firmware passes its panel's native mapping).
    /// The backdrop fill is derived here so it's consistent across hosts.
    pub fn render_frame<D, F>(
        &mut self,
        target: &mut D,
        reader: &Reader,
        w: f64,
        h: f64,
        color_fn: F,
    ) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let vp = self.state.viewport(w, h);
        let bg_rgb565 = reader.backdrop_style().map_or(DEFAULT_BG_RGB565, |s| s.color);
        let bg = color_fn(bg_rgb565);
        // Pass `color_fn` by reference so the marker overlay can reuse it after the
        // map render (`&F: Fn` when `F: Fn`), keeping its quantization consistent.
        let stats = self.renderer.render(target, reader, &vp, bg, &color_fn);

        // Overlay the user-position marker on top of the map. The geometry lives in
        // the shared renderer; this is the only glue between `AppState` and it.
        if let Some(fix) = self.state.user_fix {
            let marker_color = color_fn(reader.marker_color);
            self.renderer.draw_marker(target, &vp, fix.lon, fix.lat, fix.course, marker_color);
        }

        stats
    }
}
