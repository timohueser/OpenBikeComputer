//! [`AppState`] — the device's view state — and [`App`], the shared per-frame
//! driver that both hosts run.

use embedded_graphics::draw_target::DrawTarget;
use obcm_reader::Reader;
use obcm_render::{zoom_for_mpp, MapRenderer, RenderStats, Viewport};
use obcm_route::{Profile, RouteMatch, RouteReader};

use crate::activity::{Activity, Mode};
use crate::hal::{Fix, InputClock, InputSource, LocationSource, RideClock, Sensors};
use crate::input::{Gesture, Gestures, DEFAULT_HOLD_MS};
use crate::route::{Catalog, RouteSummary};
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

/// The screen-space axis the encoder pans along in [pan mode](Pan).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanAxis {
    /// Up / down in screen space — the default on entering pan.
    Vertical,
    /// Left / right in screen space.
    Horizontal,
}

impl PanAxis {
    /// The other axis — encoder `press` toggles between the two.
    pub fn toggled(self) -> Self {
        match self {
            PanAxis::Vertical => PanAxis::Horizontal,
            PanAxis::Horizontal => PanAxis::Vertical,
        }
    }

    /// Unit screen-space direction a **positive** detent pans the camera centre
    /// toward: vertical → up (`-y`), horizontal → right (`+x`).
    fn unit(self) -> (f32, f32) {
        match self {
            PanAxis::Vertical => (0.0, -1.0),
            PanAxis::Horizontal => (1.0, 0.0),
        }
    }
}

/// Active **pan-mode** state. While this is `Some`, the camera is detached
/// ([`Free`](CameraMode::Free)) and frozen where the rider left it: GPS fixes no
/// longer recenter it, and the map rotation is locked to
/// [`frozen_course_rad`](Pan::frozen_course_rad) so a live heading update can't spin
/// the map under the pan. `None` = the normal Follow map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pan {
    /// Which screen axis `turn` pans along.
    pub axis: PanAxis,
    /// `true` = north-up; `false` = heading-up at
    /// [`frozen_course_rad`](Pan::frozen_course_rad). Encoder `hold` toggles it.
    pub north_up: bool,
    /// The frozen heading-up rotation (radians CW from north), snapshotted on entry
    /// and re-snapshotted whenever `hold` flips back to heading-up — so the map never
    /// rotates *while* you pan.
    pub frozen_course_rad: f32,
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
    /// Pan mode, or `None` on the normal Follow map. `Some` detaches the camera and
    /// freezes the rotation (see [`Pan`]); the Map screen binds the encoder/Back to
    /// panning while it's set and draws the pan HUD over the map.
    pub pan: Option<Pan>,
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
            pan: None,
        }
    }

    /// Advance one tick: poll the location source and, in
    /// [`Follow`](CameraMode::Follow) mode, recenter the camera on the new fix.
    /// In [`Free`](CameraMode::Free) mode the fix is still recorded (for the
    /// marker) but the camera is left wherever the host's pan/zoom put it.
    ///
    /// No fix this tick leaves everything untouched, so a momentary GPS dropout
    /// holds the last camera position rather than snapping anywhere.
    ///
    /// Returns the new [`Fix`] when one arrived this tick (so [`App::tick`] can feed it to
    /// the map-matcher and ride accumulators), or `None` when there was no fresh fix.
    pub fn update(&mut self, loc: &mut dyn LocationSource) -> Option<Fix> {
        let fix = loc.poll()?;
        self.user_fix = Some(fix);
        // Recenter only when actually following — pan mode runs in Free, but guard on
        // `pan` too so a frozen camera can never be yanked back by an incoming fix.
        if self.mode == CameraMode::Follow && self.pan.is_none() {
            self.cam_lon = fix.lon;
            self.cam_lat = fix.lat;
        }
        Some(fix)
    }

    /// Project the current camera into a [`Viewport`] for a `w`×`h` pixel display.
    /// The host supplies its own dimensions, so the same state renders correctly
    /// to the 240×320 device panel and to a resizable simulator window.
    ///
    /// In [`heading_up`](AppState::heading_up) mode the projection is rotated so
    /// the last fix's `course` points to the top of the screen; with no course (or
    /// north-up) it stays north-up.
    pub fn viewport(&self, w: f32, h: f32) -> Viewport {
        Viewport::new_rotated(w, h, self.cam_lon, self.cam_lat, self.zoom, self.course_rad())
    }

    /// The rotation (radians CW from north) the projection puts at screen-up. In
    /// [pan mode](Pan) it's the frozen pan angle (0 north-up, else the snapshot); on
    /// the normal map it's the live fix course when [`heading_up`](AppState::heading_up),
    /// else north-up. Shared by [`viewport`](AppState::viewport) and the pan math so
    /// the two never disagree.
    pub(crate) fn course_rad(&self) -> f32 {
        match self.pan {
            Some(pan) if pan.north_up => 0.0,
            Some(pan) => pan.frozen_course_rad,
            None if self.heading_up => self.live_course_rad(),
            None => 0.0,
        }
    }

    /// The heading-up angle to freeze from the latest fix right now (0 when stopped /
    /// no fix). Used on entering pan and when `hold` flips back to heading-up.
    fn live_course_rad(&self) -> f32 {
        self.user_fix.and_then(|f| f.course).map_or(0.0, |deg| deg.to_radians())
    }

    /// Switch to the **riding view** — what loading a route should look like on the
    /// device: follow the user, heading-up, and zoomed in close ([`RIDING_MPP`] m/px,
    /// a ~120 m-wide view on the 240 px panel). The camera is seeded at `(lon, lat)`
    /// (the route start) so the first frame is sensible; Follow mode then recenters it
    /// on each GPS fix.
    pub fn enter_riding_view(&mut self, lon: i32, lat: i32) {
        self.mode = CameraMode::Follow;
        self.heading_up = true;
        self.pan = None;
        self.cam_lon = lon;
        self.cam_lat = lat;
        self.zoom = zoom_for_mpp(RIDING_MPP);
    }

    /// Enter **pan mode**: detach the camera ([`Free`](CameraMode::Free)) so fixes stop
    /// recentering it, and snapshot the current orientation (keeping north-up vs
    /// heading-up) with its angle frozen. Axis starts [`Vertical`](PanAxis::Vertical).
    pub fn enter_pan(&mut self) {
        self.mode = CameraMode::Free;
        self.pan = Some(Pan {
            axis: PanAxis::Vertical,
            north_up: !self.heading_up,
            frozen_course_rad: self.live_course_rad(),
        });
    }

    /// Leave pan mode: drop the pan state, resume [`Follow`](CameraMode::Follow), and
    /// recenter on the last fix so the rider snaps straight back onto themselves.
    pub fn exit_pan(&mut self) {
        self.pan = None;
        self.mode = CameraMode::Follow;
        self.recenter_on_user();
    }

    /// Recenter the camera on the last known fix — encoder `back` in pan mode (which
    /// stays in pan). No-op before the first fix.
    pub fn recenter_on_user(&mut self) {
        if let Some(fix) = self.user_fix {
            self.cam_lon = fix.lon;
            self.cam_lat = fix.lat;
        }
    }

    /// Toggle the active pan axis (encoder `press`). No-op when not panning.
    pub fn toggle_pan_axis(&mut self) {
        if let Some(pan) = self.pan.as_mut() {
            pan.axis = pan.axis.toggled();
        }
    }

    /// Toggle north-up ↔ heading-up while panning (encoder `hold`), re-freezing the
    /// heading-up angle from the latest fix so it tracks the rider's current heading.
    /// No-op when not panning.
    pub fn toggle_pan_orientation(&mut self) {
        let frozen = self.live_course_rad();
        if let Some(pan) = self.pan.as_mut() {
            pan.north_up = !pan.north_up;
            if !pan.north_up {
                pan.frozen_course_rad = frozen;
            }
        }
    }

    /// Pan the camera by `detents` encoder steps along the active axis
    /// ([`PAN_STEP_PX`] each). No-op when not panning.
    pub fn pan_step(&mut self, detents: i32) {
        let Some(pan) = self.pan else { return };
        let (ux, uy) = pan.axis.unit();
        let d = detents as f32 * PAN_STEP_PX;
        self.pan_by_pixels(ux * d, uy * d);
    }

    /// Shift the camera centre by a screen-space pixel offset, honouring the current
    /// zoom, latitude aspect, and frozen rotation. Reuses [`Viewport::to_map`] on a
    /// zero-sized viewport — the screen centre cancels out of the inverse projection,
    /// so this needs no display dimensions and the projection math stays in one place.
    fn pan_by_pixels(&mut self, dx: f32, dy: f32) {
        let vp = Viewport::new_rotated(0.0, 0.0, self.cam_lon, self.cam_lat, self.zoom, self.course_rad());
        let (lon, lat) = vp.to_map(dx, dy);
        self.cam_lon = lon;
        self.cam_lat = lat;
    }
}

/// Ground meters-per-pixel to zoom to when a route loads — close enough for
/// turn-by-turn riding rather than the whole-route overview.
const RIDING_MPP: f32 = 0.5;

/// Camera travel **per encoder detent** in pan mode, in screen pixels. A *screen*
/// amount (not ground metres), so panning is finer when zoomed in — "panning always
/// happens at the current zoom level". The single knob for pan speed; tune here.
pub const PAN_STEP_PX: f32 = 40.0;

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
///     // GPS + barometer + active route → camera, map-match, ride stats.
///     let sensors = Sensors { loc: &mut location_source, altimeter: Some(&mut baro) };
///     app.tick(RideClock(now_ms), sensors, route.as_ref());
///     app.handle_input(InputClock(now_ms), &mut input_source); // encoder + Back → gestures
///     app.render_frame(&mut display, &reader, route.as_ref(), w, h, color_policy);
/// }
/// ```
pub struct App {
    /// The camera / orientation / last-fix state — public so the host's mouse
    /// pan/zoom and control panel can read and adjust it directly (the Map screen
    /// renders from the very same state).
    pub state: AppState,
    /// The ride mode + (later) tracking accumulators.
    pub activity: Activity,
    /// The resident route catalog (summaries), populated by the host from its store
    /// ([`set_routes`](App::set_routes)). The Route menu lists it; `active_route`
    /// indexes it.
    catalog: Catalog,
    /// The screen stack (root = Home). The top screen receives input; drawing
    /// starts from the topmost opaque screen so overlays composite over the map.
    stack: Stack,
    /// The active route's resident elevation profile, rebuilt on route load (it streams
    /// every chunk, so never per frame) and handed to the Statistics screen via
    /// [`Render`]. `None` when no route is loaded; [`profile_route`](App::profile_route)
    /// tracks which route it was built for.
    profile: Option<Profile>,
    /// The [`active_route`](Activity::active_route) the cached [`profile`](App::profile)
    /// was built for, so a route change triggers exactly one rebuild.
    profile_route: Option<usize>,
    /// The live route-matcher (snaps each GPS fix to the active route → progress /
    /// off-route). Reset on route change; runs in [`tick`](App::tick), result stored on
    /// [`Activity`].
    route_match: RouteMatch,
    /// The [`active_route`](Activity::active_route) the matcher + ride accumulators were
    /// last reset for, so loading/swapping a route restarts tracking exactly once.
    ride_route: Option<usize>,
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
    /// Build the app straight onto the live map: the stack is `[Home, Map]`, with Home
    /// the always-present root that Finish / Discard return to, and no route loaded — the
    /// map shows by itself until one is picked. This is the map-first constructor the
    /// simulator uses for headless `--png` renders (and the tests); the interactive GUI
    /// and the device both boot via [`new_idle`](App::new_idle) (Home / Idle).
    pub fn new(state: AppState) -> Self {
        let mut app = Self::new_idle(state);
        app.activity = Activity::new(Mode::Riding);
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
            catalog: Catalog::new(),
            stack,
            profile: None,
            profile_route: None,
            route_match: RouteMatch::new(),
            ride_route: None,
            renderer: MapRenderer::new(),
            gestures: Gestures::new(DEFAULT_HOLD_MS),
            last_gesture: None,
            now_ms: 0,
            enc_progress: 0.0,
            back_progress: 0.0,
        }
    }

    /// Advance one tick from the sensors.
    ///
    /// Polls the GPS [`LocationSource`] (recenters the camera in Follow mode) and, when a
    /// route is loaded, snaps the new fix onto it with the resident [`RouteMatch`] and
    /// integrates the actually-ridden distance / moving time. Separately polls the
    /// barometric [`AltimeterSource`] (when present) and integrates climb — the two
    /// sensor streams are asynchronous, so each accumulates on its own cadence and a
    /// missing fix or baro sample simply contributes nothing this tick.
    ///
    /// `clock` is the [`RideClock`] — fix-consistent millis (wall-clock on the device, GPX
    /// playback-time in the sim) used for moving-time, so Avg. Speed isn't scaled by the
    /// replay multiplier. (Button hold-timing uses the separate [`InputClock`] in
    /// [`handle_input`](App::handle_input).) The polled sensors arrive bundled in
    /// [`Sensors`].
    ///
    /// Loading or swapping a route (a change in [`Activity::active_route`]) resets the
    /// matcher and ride totals here, so tracking starts fresh exactly once per load.
    pub fn tick(&mut self, clock: RideClock, sensors: Sensors, route: Option<&RouteReader>) {
        let now_ms = clock.0;
        // A new route → restart tracking (matcher + accumulators) exactly once.
        if self.activity.active_route != self.ride_route {
            self.route_match.reset();
            self.activity.reset_ride();
            self.ride_route = self.activity.active_route;
        }
        // Mirror the active route's length for the riding views (0 when none loaded).
        self.activity.route_total_m = route.map_or(0, |r| r.total_distance_m);

        let Sensors { loc, altimeter } = sensors;
        // GPS fix → camera + map-match + ridden distance/time (only on a fresh fix, so a
        // dropout doesn't re-run the matcher or double-count on a stale position).
        if let Some(fix) = self.state.update(loc) {
            if let Some(route) = route {
                let m = self.route_match.update(fix.lon, fix.lat, route);
                self.activity.apply_match(m);
            }
            self.activity.record_motion(fix, now_ms);
        }

        // Barometric altitude (its own cadence) → actually-ridden climb.
        if let Some(altimeter) = altimeter {
            if let Some(alt) = altimeter.poll() {
                self.activity.record_altitude(alt);
            }
        }
    }

    /// Replace the resident route catalog from the host's store (the simulator's
    /// folder scan / the firmware's SD-card scan). Clones up to
    /// [`MAX_ROUTES`](crate::MAX_ROUTES) summaries; any beyond that are ignored.
    pub fn set_routes(&mut self, summaries: &[RouteSummary]) {
        self.catalog.clear();
        for s in summaries.iter().take(crate::route::MAX_ROUTES) {
            let _ = self.catalog.push(s.clone());
        }
    }

    /// The resident route catalog.
    pub fn routes(&self) -> &[RouteSummary] {
        &self.catalog
    }

    /// Drain raw control input through the shared recognizer and dispatch each
    /// resulting gesture to the top screen, applying the navigation transition it
    /// returns. `clock` is the [`InputClock`] (host/MCU wall-clock millis) for hold
    /// timing. Call once per frame even with no pending events — that is how a held
    /// button's long-press fires at its threshold.
    pub fn handle_input(&mut self, clock: InputClock, input: &mut dyn InputSource) {
        let now_ms = clock.0;
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
        let App { state, activity, catalog, stack, now_ms, .. } = self;
        let mut cx = Ctx { state, activity, routes: catalog.as_slice(), now_ms: *now_ms };
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
        route: Option<&RouteReader>,
        w: f32,
        h: f32,
        color_fn: F,
    ) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        // Rebuild the cached elevation profile when the active route changes — it
        // streams every chunk, so it's built here once on load, never per frame. Keyed
        // on the active-route index (same simplification as the host's route reload):
        // it clears when no route is loaded.
        if self.activity.active_route != self.profile_route {
            self.profile = route.map(|r| r.elevation_profile());
            self.profile_route = self.activity.active_route;
        }

        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        let App { state, activity, catalog, renderer, stack, now_ms, enc_progress, profile, .. } = self;
        let mut rx = Render {
            reader,
            renderer,
            state,
            activity,
            routes: catalog.as_slice(),
            route,
            profile: profile.as_ref(),
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
