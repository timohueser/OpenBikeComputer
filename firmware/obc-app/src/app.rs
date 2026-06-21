//! [`AppState`] — the device's view state — and [`App`], the shared per-frame
//! driver that both hosts run.

use embedded_graphics::draw_target::DrawTarget;
use obc_reader::Reader;
use obc_render::{zoom_for_mpp, MapRenderer, RenderStats, Viewport};
use obc_route::{Profile, RouteMatch, RouteReader, TrackPoint};

use crate::activity::{Activity, Mode};
use crate::breadcrumb::Breadcrumb;
use crate::dirty::Dirty;
use crate::hal::{Fix, InputClock, InputSource, LocationSource, RideClock, Sensors};
use crate::hold_hint::HoldHints;
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
/// the [`obc_render::MapRenderer`]/draw target; each frame it calls [`update`] with the
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
        let vp = Viewport::new_rotated(
            0.0,
            0.0,
            self.cam_lon,
            self.cam_lat,
            self.zoom,
            self.course_rad(),
        );
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
    /// The [`active_route`](Activity::active_route) the **matcher** was last reset for, so
    /// changing the navigated route — a load *or* a "Swap route only" — re-locks it once.
    matched_route: Option<usize>,
    /// The [`session`](Activity::session) the **ride accumulators + breadcrumb** were last
    /// reset for, so a new tracking session (load from Idle / "Save & start new") restarts
    /// them once, while a swap (same session) leaves them running.
    ride_session: Option<u32>,
    /// The travelled-path breadcrumb (RAM, bounded), fed each logged fix in
    /// [`tick`](App::tick) and drawn on the Map; cleared when `ride_session` changes.
    breadcrumb: Breadcrumb,
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
    /// The global long-press hint overlay (charge-in-place pills at the encoder /
    /// Back edges), drawn above every screen so the central hold gesture is always
    /// visible — not just on Ride control.
    hold_hints: HoldHints,
    /// Accumulated **map-plane** repaint demand since the last [`take_dirty`](App::take_dirty):
    /// set as [`tick`](App::tick) / [`handle_input`](App::handle_input) mutate map-affecting
    /// state, drained once per frame. Starts `true` so the host's first frame paints. (The
    /// overlay-plane flag isn't accumulated here — it's derived from the live hold-bulge state
    /// at drain time; see [`take_dirty`](App::take_dirty).)
    map_dirty: bool,
    /// The overlay plane's live state at the previous [`take_dirty`](App::take_dirty), so a
    /// bulge going quiet yields exactly one trailing overlay repaint (clearing the last frame
    /// off Layer 2). Folds the host's old `overlay_was_active` bookkeeping into the shared
    /// [`Dirty`] signal so the sim and firmware share one definition.
    overlay_was_active: bool,
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
            matched_route: None,
            ride_session: None,
            breadcrumb: Breadcrumb::new(),
            renderer: MapRenderer::new(),
            gestures: Gestures::new(DEFAULT_HOLD_MS),
            last_gesture: None,
            now_ms: 0,
            enc_progress: 0.0,
            back_progress: 0.0,
            hold_hints: HoldHints::new(),
            // Force the host's first frame: nothing has been drawn yet, so the map is dirty.
            map_dirty: true,
            overlay_was_active: false,
        }
    }

    /// Advance one tick from the sensors.
    ///
    /// Polls the GPS [`LocationSource`] (recenters the camera in Follow mode) and, with a
    /// route loaded, snaps the fix onto it via [`RouteMatch`] and integrates ridden distance
    /// / moving time. Separately polls the barometer for climb — the two streams are
    /// asynchronous, so each accumulates on its own cadence and a missing sample just
    /// contributes nothing this tick. Sensors arrive bundled in [`Sensors`].
    ///
    /// `clock` is the [`RideClock`] (fix-consistent millis — wall-clock on device, playback
    /// time in the sim) so moving-time isn't scaled by the replay multiplier; button holds use
    /// the separate [`InputClock`] in [`handle_input`](App::handle_input). Loading or swapping
    /// a route ([`Activity::active_route`] change) resets the matcher and ride totals here,
    /// once per load.
    pub fn tick(&mut self, clock: RideClock, sensors: Sensors, route: Option<&RouteReader>) {
        let now_ms = clock.0;
        // The camera state before this tick's fix, so a fresh fix that actually moved the
        // camera / marker / heading is detected below by one `AppState` comparison (it's
        // `Copy` + `PartialEq`).
        let state_before = self.state;
        // The matcher follows the *navigated route*: a load or a "Swap route only" re-locks it.
        if self.activity.active_route != self.matched_route {
            self.route_match.reset();
            self.matched_route = self.activity.active_route;
            self.map_dirty = true; // route load / swap repaints the route line + recenters
        }
        // The accumulators + breadcrumb follow the *tracking session*: a new session restarts
        // them, while a swap (which keeps the session) leaves them running.
        if self.activity.session != self.ride_session {
            self.activity.reset_ride();
            self.breadcrumb.clear();
            self.ride_session = self.activity.session;
            self.map_dirty = true; // the breadcrumb cleared — the map's travelled trail changed
        }
        // Mirror the active route's length for the riding views (0 when none loaded). A change
        // here means the *drawable* route just appeared or vanished — a load, or (on the device)
        // a transient SD glitch recovering, where the geometry becomes streamable a frame or two
        // after the load. Dirty the map so the route line is painted (or cleared) even on a frame
        // with no fresh fix to trigger it, closing an under-redraw gap independent of `active_route`.
        let route_total_before = self.activity.route_total_m;
        self.activity.route_total_m = route.map_or(0, |r| r.total_distance_m);
        if self.activity.route_total_m != route_total_before {
            self.map_dirty = true;
        }

        let Sensors { loc, altimeter, track } = sensors;
        // Barometric altitude on its own cadence → climb + the elevation stamped on the log.
        // Polled before the fix so a point logged this tick carries the freshest altitude.
        if let Some(altimeter) = altimeter {
            if let Some(alt) = altimeter.poll() {
                self.activity.record_altitude(alt);
            }
        }
        // GPS fix → camera + map-match + ridden distance/time (only on a fresh fix, so a
        // dropout doesn't re-run the matcher or double-count). A *logged* fix also feeds the
        // breadcrumb + the ride log.
        if let Some(fix) = self.state.update(loc) {
            if let Some(route) = route {
                let m = self.route_match.update(fix.lon, fix.lat, route);
                self.activity.apply_match(m);
            }
            let motion = self.activity.record_motion(fix, now_ms);
            if motion.log {
                self.breadcrumb.push(fix.lon, fix.lat);
                if let Some(track) = track {
                    track.record(TrackPoint {
                        lon: fix.lon,
                        lat: fix.lat,
                        ele: self.activity.track_ele(),
                        t_ms: now_ms,
                        segment_start: motion.segment_start,
                    });
                }
            }
        }
        // A fresh fix that actually moved the camera, marker or heading dirties the map — but
        // only on a screen that *draws* live data (the Map / Statistics riding views). On the
        // Home screensaver and the menus the camera still follows the fix, yet nothing they draw
        // uses it, so a fix there must not redraw them (the "static Home does zero map renders"
        // criterion). The `AppState` comparison also makes a stationary fix that changed nothing
        // a no-op. (The breadcrumb only grows on a *moving* logged fix, which moved `user_fix`
        // too, so it's covered by this same comparison — no separate breadcrumb check needed.)
        if self.state != state_before && self.shows_live_data() {
            self.map_dirty = true;
        }
    }

    /// Whether the screen currently drawing the base view shows live sensor data (the user
    /// fix / ride accumulators) — the Map and Statistics riding views do, so a fresh fix must
    /// redraw them; the Home screensaver and the menus don't, so a fix (which still moves the
    /// camera behind them) must not. The base is the lowest *opaque* drawn screen — the same one
    /// [`render_map`](App::render_map) starts from — so an overlay (Ride control) over a riding
    /// view still counts as live, since the map keeps moving under the pause panel.
    fn shows_live_data(&self) -> bool {
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        matches!(self.stack.get(base), Some(Screen::Map(_) | Screen::Statistics(_)))
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
        // `tick` is the only source of Hold/BackHold — note which fired this frame so
        // the hint overlay pops the matching pill the instant the threshold crosses.
        let (mut enc_fired, mut back_fired) = (false, false);
        if let Some(g) = self.gestures.tick(now_ms) {
            match g {
                Gesture::Hold => enc_fired = true,
                Gesture::BackHold => back_fired = true,
                _ => {}
            }
            self.dispatch(g);
        }
        self.enc_progress = self.gestures.encoder_progress(now_ms);
        self.back_progress = self.gestures.back_progress(now_ms);
        self.hold_hints.update(
            now_ms,
            self.enc_progress,
            self.back_progress,
            enc_fired,
            back_fired,
        );
        // Advance each visible screen's time-driven content (today: the Statistics cursor's
        // spring-back to the live position) on the input clock, and dirty the map if any of it
        // changed — so a screen surfaces its own timed-refresh need rather than the host
        // re-rendering on a blind heartbeat (issue #47). Cheap: a clock comparison per drawn
        // screen, the same `base..` range `render_map` draws (so an overlay over a riding view
        // still lets the view underneath settle).
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        let mut animated = false;
        for scr in self.stack.iter_mut().skip(base) {
            animated |= scr.animate(now_ms);
        }
        self.map_dirty |= animated;
    }

    /// Route one gesture to the top screen and apply the transition it returns.
    fn dispatch(&mut self, g: Gesture) {
        self.last_gesture = Some(g);
        // Any recognized gesture drives the top screen, and every screen — the map, the
        // menus, the Ride-control overlay — renders into the map plane (Layer 1), so a
        // dispatched gesture dirties it. Conservative by design (a gesture a screen ignores
        // still costs one redraw), which is what keeps the idle path exact: no gesture is
        // recognized, so `dispatch` never runs and the map stays clean — zero idle renders.
        self.map_dirty = true;
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
    ///
    /// This is the single-target convenience that draws a whole frame:
    /// [`render_map`](App::render_map) then [`render_overlay`](App::render_overlay)
    /// into the *same* target, in that order, so the result is byte-identical to the
    /// old monolithic path. Hosts that keep the map and overlay on separate
    /// buffers/layers (dual-layer display) call the two halves directly instead.
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
        let stats = self.render_map(target, reader, route, w, h, &color_fn);
        self.render_overlay(target, w, h, &color_fn);
        stats
    }

    /// Render **only the map plane** — the screen stack from the topmost opaque
    /// screen upward (map + screen content, incl. the Ride-control overlay screen),
    /// but **excluding** the global hold-hint chrome. Returns the map
    /// [`RenderStats`] for the host's stats panel.
    ///
    /// This is the expensive half (24–51 ms on the device); a host that keeps the
    /// transient overlay on its own buffer/layer renders this only when the map
    /// actually changed, then repaints the cheap [`render_overlay`](App::render_overlay)
    /// over it at a higher rate. `color_fn` is the display-specific RGB565 mapping
    /// (see [`render_frame`](App::render_frame)).
    pub fn render_map<D, F>(
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
        let App {
            state,
            activity,
            catalog,
            renderer,
            stack,
            now_ms,
            enc_progress,
            profile,
            breadcrumb,
            ..
        } = self;
        let mut rx = Render {
            reader,
            renderer,
            state,
            activity,
            routes: catalog.as_slice(),
            route,
            profile: profile.as_ref(),
            breadcrumb: &*breadcrumb,
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

    /// Render **only the overlay plane** — the transient always-on-top chrome (the
    /// global long-press hint / confirm bulge), over whatever is already in `target`.
    ///
    /// **Compositing contract** (so this can later live on its own buffer/layer):
    /// `render_overlay` paints *only* its own pixels — the hold-bulge strips — and
    /// **never** clears or otherwise touches the rest of the target. It must be valid
    /// drawn over arbitrary existing content, so a host can repaint it over an
    /// unchanged map without re-running [`render_map`](App::render_map). Poll
    /// [`overlay_active`](App::overlay_active) to decide whether a repaint is needed.
    ///
    /// On the simulator (and today's single-buffer firmware) this draws directly over
    /// the map buffer, so non-overlay pixels simply keep the map underneath. On the
    /// device's dedicated overlay layer the non-overlay pixels are *transparent*; the
    /// exact convention (per-pixel alpha vs. chroma-key) is finalised in the
    /// dual-layer display issue. The bulge is drawn opaque in `palette::HUD`, so it
    /// needs no alpha and reads identically on the 8-colour panel.
    pub fn render_overlay<D, F>(&self, target: &mut D, w: f32, h: f32, color_fn: F)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        self.hold_hints.draw(target, &color_fn, w as i32, h as i32, self.now_ms);
    }

    /// Whether the overlay plane has live content this frame — a hold bulge that is
    /// charging, popping, or retracting. A host driving the map and overlay as
    /// separate layers polls this to decide when [`render_overlay`](App::render_overlay)
    /// must repaint over an unchanged map; it is `false` exactly when the overlay
    /// would draw nothing, so the overlay layer can stay idle.
    pub fn overlay_active(&self) -> bool {
        self.hold_hints.active(self.now_ms)
    }

    /// Drain the repaint demand accumulated since the last call, resetting to
    /// [`Dirty::CLEAN`]. The host calls this **once per frame** after [`tick`](App::tick) +
    /// [`handle_input`](App::handle_input), then renders [`render_map`](App::render_map) only
    /// when [`Dirty::map`] and [`render_overlay`](App::render_overlay) only when
    /// [`Dirty::overlay`] — the render-on-demand loop (issue #47).
    ///
    /// [`map`](Dirty::map) is the accumulation of every map-affecting mutation since the last
    /// drain (a dispatched gesture, a camera-moving fix on a riding view, a route/session change,
    /// a screen's timed `animate`). [`overlay`](Dirty::overlay) is *derived* from the live
    /// hold-bulge state rather than accumulated: it's set while [`overlay_active`](App::overlay_active)
    /// is true, plus exactly one trailing frame after the bulge goes quiet so the host can clear
    /// it off Layer 2. Because that trailing edge is tracked across calls, draining twice in one
    /// frame would swallow it — call exactly once per frame.
    pub fn take_dirty(&mut self) -> Dirty {
        let overlay_now = self.overlay_active();
        let dirty = Dirty { map: self.map_dirty, overlay: overlay_now || self.overlay_was_active };
        self.map_dirty = false;
        self.overlay_was_active = overlay_now;
        dirty
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
