//! [`AppState`] — the device's view state — and [`App`], the shared per-frame
//! driver that both hosts run.

use embedded_graphics::draw_target::DrawTarget;
use obcm_reader::Reader;
use obcm_render::{MapRenderer, RenderStats, Viewport};

use crate::activity::{Activity, Mode};
use crate::hal::{Fix, InputSource, LocationSource};
use crate::input::{Gesture, Gestures, DEFAULT_HOLD_MS};
use crate::screen::{self, Ctx, HomeScreen, MapScreen, Render, Screen, Stack};

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
/// the [`obcm_render::MapRenderer`]/draw target; each frame it calls [`update`] with the
/// platform's [`LocationSource`], then [`viewport`] to get the camera to render
/// through. The split keeps display dimensions (240×320 on the device, a resized
/// window on the host) out of the shared state.
///
/// [`update`]: AppState::update
/// [`viewport`]: AppState::viewport
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppState {
    /// Camera center longitude in microdegrees (1e-6°).
    pub cam_lon: i32,
    /// Camera center latitude in microdegrees (1e-6°).
    pub cam_lat: i32,
    /// Pixels per microdegree of latitude (the [`Viewport::zoom`] convention).
    pub zoom: f32,
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
    pub fn new(cam_lon: i32, cam_lat: i32, zoom: f32) -> Self {
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
                self.cam_lon = fix.lon;
                self.cam_lat = fix.lat;
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
    pub fn viewport(&self, w: f32, h: f32) -> Viewport {
        let course_rad = if self.heading_up {
            self.user_fix
                .and_then(|f| f.course)
                .map_or(0.0, |deg| deg.to_radians())
        } else {
            0.0
        };
        Viewport::new_rotated(w, h, self.cam_lon, self.cam_lat, self.zoom, course_rad)
    }
}

/// The whole device application, ready to run a frame.
///
/// The single entry point both hosts share: the simulator and the firmware each
/// construct one `App`, then per frame [`tick`](App::tick) it with their
/// platform's [`LocationSource`], feed raw controls through
/// [`handle_input`](App::handle_input) with their [`InputSource`] + millis clock,
/// and [`render_frame`](App::render_frame) it to their display. `App` owns the
/// screen stack, the shared gesture recognizer, the camera [`AppState`], the ride
/// [`Activity`], and the reusable [`MapRenderer`]; each frame it runs
/// poll-inputs → top-screen `handle` → apply `Transition` → draw the stack.
///
/// ```ignore
/// let mut app = App::new(AppState::new(cx, cy, zoom));
/// loop {
///     app.tick(&mut location_source);              // GPS / control panel / GPX
///     app.handle_input(now_ms, &mut input_source); // encoder + Back → gestures
///     app.render_frame(&mut display, &reader, w, h, color_policy);
/// }
/// ```
pub struct App {
    /// The camera / orientation / last-fix state — public so the host's mouse
    /// pan/zoom and control panel can read and adjust it directly (the Map screen
    /// renders from the very same state).
    pub state: AppState,
    /// The ride mode + (later) tracking accumulators.
    pub activity: Activity,
    /// The screen stack (root = Home). The top screen receives input; drawing
    /// starts from the topmost opaque screen so overlays composite over the map.
    stack: Stack,
    /// Reused renderer; clears (not frees) its scratch each frame, so steady-state
    /// rendering does no allocation — important on the MCU.
    renderer: MapRenderer,
    /// The shared gesture recognizer (raw events + clock → the five gestures).
    gestures: Gestures,
    /// The most recently recognized gesture, for the host's input readout.
    last_gesture: Option<Gesture>,
    /// Millis at the last [`handle_input`](App::handle_input), passed to draw.
    now_ms: u32,
    /// In-flight encoder / Back hold-progress (0.0–1.0) for the confirm ring.
    enc_progress: f32,
    back_progress: f32,
}

impl App {
    /// Build the app on the live map, as if route 0 were already loaded — the
    /// simulator's convenience default (it opens on the map; mouse/GPX/`--png` all
    /// work). The stack is `[Home, Map]`, with Home the always-present root that
    /// Finish / Discard return to. Use [`new_idle`](App::new_idle) for the device's
    /// real boot (start at Home / Idle, no route).
    pub fn new(state: AppState) -> Self {
        let mut app = Self::new_idle(state);
        app.activity = Activity { mode: Mode::Riding, active_route: Some(0) };
        let _ = app.stack.push(Screen::Map(MapScreen::new()));
        app
    }

    /// Build the app at the device's real power-on state: the Home screensaver,
    /// Idle, no route loaded. Loading a route (Home → Route menu → `press`) starts
    /// riding and opens the Map.
    pub fn new_idle(state: AppState) -> Self {
        let mut stack = Stack::new();
        let _ = stack.push(Screen::Home(HomeScreen::new()));
        App {
            state,
            activity: Activity::new(Mode::Idle),
            stack,
            renderer: MapRenderer::new(),
            gestures: Gestures::new(DEFAULT_HOLD_MS),
            last_gesture: None,
            now_ms: 0,
            enc_progress: 0.0,
            back_progress: 0.0,
        }
    }

    /// Advance one tick from the location source (recenters the camera on the new
    /// fix in Follow mode).
    pub fn tick(&mut self, loc: &mut dyn LocationSource) {
        self.state.update(loc);
    }

    /// Drain raw control input through the shared recognizer and dispatch each
    /// resulting gesture to the top screen, applying the navigation transition it
    /// returns. `now_ms` is the host/MCU millis clock. Call once per frame even
    /// with no pending events — that is how a held button's long-press fires at
    /// its threshold.
    pub fn handle_input(&mut self, now_ms: u32, input: &mut dyn InputSource) {
        self.now_ms = now_ms;
        while let Some(ev) = input.poll() {
            if let Some(g) = self.gestures.on_event(ev, now_ms) {
                self.dispatch(g);
            }
        }
        if let Some(g) = self.gestures.tick(now_ms) {
            self.dispatch(g);
        }
        self.enc_progress = self.gestures.encoder_progress(now_ms);
        self.back_progress = self.gestures.back_progress(now_ms);
    }

    /// Route one gesture to the top screen and apply the transition it returns.
    fn dispatch(&mut self, g: Gesture) {
        self.last_gesture = Some(g);
        let App { state, activity, stack, now_ms, .. } = self;
        let mut cx = Ctx { state, activity, now_ms: *now_ms };
        let t = stack.last_mut().expect("the stack always has the Home root").handle(g, &mut cx);
        screen::apply(stack, t);
    }

    /// Render the current screen and any overlays above it into `target`, a
    /// `w`×`h` pixel display. Draws from the topmost *opaque* screen upward, so an
    /// overlay (Ride control) composites over the still-visible map. Returns the
    /// map [`RenderStats`] for the host's stats panel.
    ///
    /// `color_fn` maps a style's RGB565 to the target's pixel color — the one
    /// genuinely display-specific policy (the simulator picks true-color vs.
    /// device-64 quantization; the firmware passes its panel's native mapping).
    pub fn render_frame<D, F>(
        &mut self,
        target: &mut D,
        reader: &Reader,
        w: f32,
        h: f32,
        color_fn: F,
    ) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        let App { state, activity, renderer, stack, now_ms, enc_progress, .. } = self;
        let mut rx = Render {
            reader,
            renderer,
            state,
            activity,
            w,
            h,
            now_ms: *now_ms,
            hold_progress: *enc_progress,
        };
        let mut stats = RenderStats::default();
        for (i, scr) in stack.iter().enumerate().skip(base) {
            let s = scr.draw(target, &mut rx, &color_fn);
            if i == base {
                stats = s;
            }
        }
        stats
    }

    /// The most recently recognized gesture (host input readout), if any.
    pub fn last_gesture(&self) -> Option<Gesture> {
        self.last_gesture
    }

    /// In-flight encoder hold-progress (0.0–1.0) for the confirm-ring readout.
    pub fn encoder_hold_progress(&self) -> f32 {
        self.enc_progress
    }

    /// In-flight Back hold-progress (0.0–1.0).
    pub fn back_hold_progress(&self) -> f32 {
        self.back_progress
    }

    /// The current operating mode.
    pub fn mode(&self) -> Mode {
        self.activity.mode
    }
}
