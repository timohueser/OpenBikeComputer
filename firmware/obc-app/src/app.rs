//! [`AppState`] — the device's view state — and [`App`], the shared per-frame
//! driver that both hosts run.

use embedded_graphics::draw_target::DrawTarget;
use obc_reader::Reader;
use obc_render::{zoom_for_mpp, Clock, MapRenderer, NoopClock, RenderStats, Viewport};
use obc_route::{Profile, RouteMatch, RouteReader, TrackPoint};

use crate::activity::{Activity, Mode};
use crate::breadcrumb::Breadcrumb;
use crate::dirty::Dirty;
use crate::hal::{Fix, InputClock, InputSource, LocationSource, RideClock, Sensors};
use crate::input::Gesture;
use crate::input_plane::InputPlane;
use crate::route::{Catalog, RouteSummary};
use crate::screen::{self, Ctx, HomeScreen, MapScreen, Render, Screen, Stack};
use crate::settings::Settings;

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
    /// rotates *while* panning.
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
    /// first one. Drives the heading-up rotation and the user marker.
    pub user_fix: Option<Fix>,
    /// Pan mode, or `None` on the normal Follow map. `Some` detaches the camera and
    /// freezes the rotation (see [`Pan`]); the Map screen binds the encoder/Back to
    /// panning while it's set and draws the pan HUD over the map.
    pub pan: Option<Pan>,
    /// Latest electronic-compass heading (degrees CW from north), or `None` until one
    /// arrives. Stands in for the GPS course when the rider is stopped on a heading-up
    /// map, so the orientation follows the compass instead of snapping to north; only
    /// adopted on ticks where it would actually drive the rotation (see [`App::tick`]).
    pub compass_deg: Option<f32>,
    /// Battery charge, 0–100 %. [`App::tick`] writes it from the [`FuelGauge`](crate::FuelGauge)
    /// each tick (a fixed-value stub until the real PMIC gauge is wired); the Home screen draws
    /// the gauge from it, colouring the filled bars by level (red < 20 %, green > 80 %, amber
    /// between) and the empty bars dim grey.
    pub battery_pct: u8,
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
            compass_deg: None,
            // Stand-in until a [`FuelGauge`](crate::FuelGauge) feeds a real reading; the host's
            // boot stub (75 % on device) overwrites it on the first tick.
            battery_pct: 75,
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

    /// The heading-up angle to freeze from the latest fix right now: the GPS course, or the
    /// electronic compass when stopped (no course), or 0 (north) when neither is known. Used by
    /// [`course_rad`](AppState::course_rad), on entering pan, and when `hold` flips to heading-up.
    fn live_course_rad(&self) -> f32 {
        self.user_fix.and_then(|f| f.course).or(self.compass_deg).map_or(0.0, |deg| deg.to_radians())
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
/// amount (not ground metres), so panning is finer when zoomed in. The single knob
/// for pan speed; tune here.
pub const PAN_STEP_PX: f32 = 40.0;

/// Capacity of [`handle_input`](App::handle_input)'s per-frame gesture buffer — the
/// gestures recognised from one frame's raw input, held while `self.input` is borrowed and
/// applied after. One frame yields at most one gesture per raw event (the input queue is
/// bounded — `ButtonInput`'s is 8) plus the single per-frame long-press, so this never
/// overflows; the slack matches the cross-executor channel the firmware's two-plane path uses.
const GESTURE_BUF: usize = 16;

/// The whole device application, ready to run a frame.
///
/// The single entry point both hosts share: the simulator and the firmware each
/// construct one `App`, then per frame [`tick`](App::tick) it with their
/// platform's [`LocationSource`], feed raw controls through
/// [`handle_input`](App::handle_input) with their [`InputSource`] + millis clock,
/// and [`render_frame`](App::render_frame) it to their display. `App` owns the
/// screen stack, the input + overlay plane ([`InputPlane`]), the camera [`AppState`],
/// the ride [`Activity`], and the reusable [`MapRenderer`]; each frame it runs
/// poll-inputs → top-screen `handle` → apply `Transition` → draw the stack.
///
/// The firmware can also split the two planes across executors — recognising gestures on a
/// high-priority [`InputPlane`] that preempts the map render and feeding them back through
/// [`apply_gesture`](App::apply_gesture) (issue #48); [`handle_input`](App::handle_input) is
/// just those halves fused for the single-loop hosts.
///
/// ```ignore
/// let mut app = App::new(AppState::new(cx, cy, zoom));
/// loop {
///     // GPS + barometer + compass + active route → camera, map-match, ride stats.
///     let sensors = Sensors {
///         loc: &mut location_source,
///         altimeter: Some(&mut baro),
///         compass: Some(&mut compass),
///         track: Some(&mut track_log),
///         fuel: Some(&mut fuel_gauge),
///     };
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
    /// The ride mode + tracking accumulators.
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
    /// The input + overlay plane: the gesture recognizer, the long-press hint overlay, and
    /// the live hold-progress. Relocated off `App` into [`InputPlane`] (issue #48) so the
    /// firmware can run it on a *separate, high-priority* executor that preempts the map
    /// render — keeping input + the overlay responsive while a map frame draws. `App` keeps
    /// this one for the convenience [`handle_input`](App::handle_input) path (the sim, the
    /// single-executor firmware); the two-plane firmware drives its own standalone
    /// [`InputPlane`] and feeds the recognised gestures back through
    /// [`apply_gesture`](App::apply_gesture).
    input: InputPlane,
    /// Millis at the last [`handle_input`](App::handle_input) /
    /// [`advance_animations`](App::advance_animations) — the **map plane's** clock, passed to
    /// draw and to the [`Ctx`](screen::Ctx) a gesture is applied through. Distinct from the
    /// input plane's own clock in [`InputPlane`].
    now_ms: u32,
    /// Accumulated **map-plane** repaint demand since the last [`take_dirty`](App::take_dirty):
    /// set as [`tick`](App::tick) / [`handle_input`](App::handle_input) mutate map-affecting
    /// state, drained once per frame. Starts `true` so the host's first frame paints. (The
    /// overlay-plane flag isn't accumulated here — it's derived from the live hold-bulge state
    /// at drain time, owned by [`InputPlane`]; see [`take_dirty`](App::take_dirty).)
    map_dirty: bool,
    /// The persisted device settings, seeded from the host's store at boot
    /// ([`set_settings`](App::set_settings)) and edited in place by the settings screens.
    settings: Settings,
    /// Whether [`settings`](App::settings) changed since the last [`take_settings_dirty`]; set
    /// by [`apply_gesture`](App::apply_gesture)'s before/after compare, drained by the host to
    /// decide when to persist. Starts `false`: the boot value either came from the store or is
    /// the default, so nothing needs writing until the user actually changes something.
    ///
    /// [`take_settings_dirty`]: App::take_settings_dirty
    settings_dirty: bool,
    /// Host-supplied encoder hold-progress (0.0–1.0) for the in-screen confirm fills (the factory
    /// Reset bar; the [`RideControl`](crate::screen::RideControl) confirm rows). `None` on the
    /// single-loop hosts (the sim), where the render reads progress from `App`'s own
    /// [`InputPlane`]. The **two-plane firmware** recognises holds on a *separate* high-priority
    /// `InputPlane`, so `App`'s own one stays at `0` there; that host feeds the live progress in
    /// each frame via [`set_hold_progress`](App::set_hold_progress). Cleared back when the host
    /// reports `0` (hold released), so it tracks the live hold either way.
    hold_progress_override: Option<f32>,
    /// Millis of the last battery [`FuelGauge`](crate::FuelGauge) poll, or `None` before the
    /// first. The gauge is read on a slow cadence ([`BATTERY_POLL_MS`]) — *not* every tick — so a
    /// real PMIC read never spins the I²C bus at the frame rate; battery charge changes far too
    /// slowly to want more. See [`tick`](App::tick).
    last_battery_poll_ms: Option<u32>,
}

/// How often [`App::tick`] reads the battery [`FuelGauge`](crate::FuelGauge). Battery state of
/// charge drifts over minutes/hours, so a ~30 s cadence keeps the Home gauge fresh while reading
/// the PMIC a few times a minute at most — the per-frame poll would be pure waste (and bus
/// traffic on a real gauge). Independent of redraws: an unchanged reading repaints nothing.
const BATTERY_POLL_MS: u32 = 30_000;

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
            input: InputPlane::new(),
            now_ms: 0,
            // Force the host's first frame: nothing has been drawn yet, so the map is dirty.
            map_dirty: true,
            settings: Settings::default(),
            settings_dirty: false,
            hold_progress_override: None,
            last_battery_poll_ms: None,
        }
    }

    /// Build the idle power-on [`App`] **in place** at `slot` — the by-reference
    /// twin of [`new_idle`](App::new_idle), and the placement path the firmware uses
    /// to construct the ~200 KB resident `App` straight into its reserved memory region
    /// without ever materializing it (or its renderer scratch) on the 192 KB stack.
    ///
    /// `new_idle` returns the `App` by value, which only stays off the stack thanks to
    /// the optimizer's return-value optimization — a fragile guarantee that a debug
    /// build, a different toolchain, or a tighter target could drop, overflowing the
    /// stack (issue #67). This writes each field through `addr_of_mut!` exactly once,
    /// so no by-value `App` is ever formed: there is no stack temporary to elide. The
    /// only field big enough to matter is the renderer, zeroed in place via
    /// [`MapRenderer::init_zeroed`] rather than built-and-moved; the rest are small.
    ///
    /// The end state is identical to `new_idle`'s — keep the two in sync.
    ///
    /// # Safety
    /// `slot` must be a valid, aligned `*mut App` the caller exclusively owns and into
    /// which a full `App` may be written (e.g. the device's reserved memory region). On
    /// return the slot is fully initialized; read it via `&mut *slot`.
    pub unsafe fn init_idle(slot: *mut App, state: AppState) {
        use core::ptr::addr_of_mut;
        // SAFETY: `slot` is a valid, owned, aligned `App` region (caller's contract).
        // Every field below is written exactly once before any read, in declaration
        // order, so the slot is fully initialized on return and no field is read while
        // uninitialized.
        unsafe {
            addr_of_mut!((*slot).state).write(state);
            addr_of_mut!((*slot).activity).write(Activity::new(Mode::Idle));
            addr_of_mut!((*slot).catalog).write(Catalog::new());
            // The screen stack: empty in place, then push the always-present Home root.
            // `heapless::Vec::push` isn't `const`, so the root can't be part of a literal.
            addr_of_mut!((*slot).stack).write(Stack::new());
            let _ = (*slot).stack.push(Screen::Home(HomeScreen::new()));
            addr_of_mut!((*slot).profile).write(None);
            addr_of_mut!((*slot).profile_route).write(None);
            addr_of_mut!((*slot).route_match).write(RouteMatch::new());
            addr_of_mut!((*slot).matched_route).write(None);
            addr_of_mut!((*slot).ride_session).write(None);
            addr_of_mut!((*slot).breadcrumb).write(Breadcrumb::new());
            // The ~200 KB scratch renderer: an empty renderer *is* the all-zero bit
            // pattern, so it is zeroed straight into the slot — never on the stack.
            MapRenderer::init_zeroed(addr_of_mut!((*slot).renderer));
            addr_of_mut!((*slot).input).write(InputPlane::new());
            addr_of_mut!((*slot).now_ms).write(0);
            // Force the host's first frame: nothing has been drawn yet, so the map is dirty.
            addr_of_mut!((*slot).map_dirty).write(true);
            addr_of_mut!((*slot).settings).write(Settings::default());
            addr_of_mut!((*slot).settings_dirty).write(false);
            addr_of_mut!((*slot).hold_progress_override).write(None);
            addr_of_mut!((*slot).last_battery_poll_ms).write(None);
        }
    }

    /// Build the **map-first** [`App`] in place at `slot` — the by-reference twin of
    /// [`new`](App::new), exactly as [`init_idle`](App::init_idle) is the twin of
    /// [`new_idle`](App::new_idle). Initialises the idle power-on state, then drops straight onto
    /// the live Map (stack `[Home, Map]`, Riding) so the map shows without any navigation — the
    /// placement path a firmware bring-up uses to put the full map on glass before buttons exist
    /// (issue #125), and the in-place analog of the simulator's headless `--png` constructor.
    ///
    /// # Safety
    /// Same contract as [`init_idle`](App::init_idle): `slot` is a valid, aligned, exclusively
    /// owned `*mut App` into which a full `App` may be written. On return it is fully initialised.
    pub unsafe fn init_map(slot: *mut App, state: AppState) {
        // SAFETY: caller's contract (a valid, owned, aligned slot). `init_idle` fully initialises
        // it, so thereafter `&mut *slot` is a sound `&mut App` and the map-first tail is plain safe
        // mutation — the exact two statements `new` runs after `new_idle` (assignment, so the just
        // -written Idle activity is dropped, not leaked).
        unsafe { Self::init_idle(slot, state) };
        let app = unsafe { &mut *slot };
        app.activity = Activity::new(Mode::Riding);
        let _ = app.stack.push(Screen::Map(MapScreen::new()));
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

        let Sensors { loc, altimeter, compass, track, fuel } = sensors;
        // Battery state of charge from the PMIC fuel gauge, on a slow ~30 s cadence — *not* every
        // tick, so a real I²C read never spins at the frame rate (battery charge changes far too
        // slowly to want more). And a reading only repaints the Home screensaver — the one screen
        // that draws the gauge — when the level **actually changes**: an unchanged reading (the
        // common case, and always so for a constant stub) costs nothing and redraws nothing. The
        // `shows_live_data` gate below is for the riding views, not Home, so dirty it here.
        let battery_due = self.last_battery_poll_ms.is_none_or(|last| now_ms.wrapping_sub(last) >= BATTERY_POLL_MS);
        if battery_due {
            self.last_battery_poll_ms = Some(now_ms);
            if let Some(soc) = fuel.and_then(|f| f.poll()) {
                if soc != self.state.battery_pct {
                    self.state.battery_pct = soc;
                    let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
                    if matches!(self.stack.get(base), Some(Screen::Home(_))) {
                        self.map_dirty = true;
                    }
                }
            }
        }
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
        // Electronic compass → the heading when the GPS can't give a course. Polled after the fix
        // so it sees this tick's movement state, and adopted into `compass_deg` *only* when it
        // would actually drive the orientation: heading-up, not panning, and the latest fix has no
        // course (the rider is stopped). Storing it in any other state — moving, north-up, or
        // panning, where `course_rad` ignores it — would change `state` on every reading and force
        // a needless map redraw, breaking the render-on-demand contract (#47). When it *is*
        // adopted, the `state != state_before` check below redraws only if the heading changed.
        if let Some(compass) = compass {
            if let Some(heading) = compass.poll() {
                let stopped = self.state.user_fix.and_then(|f| f.course).is_none();
                if stopped && self.state.heading_up && self.state.pan.is_none() {
                    self.state.compass_deg = Some(heading);
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

    /// Whether the base (lowest opaque) screen actually draws the **map** — i.e. the
    /// [`Map`](crate::screen::map) screen, the only one that reads the streamed-map [`Reader`].
    /// A render-on-demand host polls this to skip the whole map pipeline on a non-map frame: don't
    /// build the `Reader` (an SD style-table parse + its stack spike) and pass `None` to
    /// [`render_map_timed`](App::render_map_timed) — a menu / Statistics / Home redraw then draws
    /// only its own chrome, with zero map I/O. (An overlay like Ride-control sits *over* the Map, so
    /// the base is still `Map` and this stays `true`.)
    pub fn base_draws_map(&self) -> bool {
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        matches!(self.stack.get(base), Some(Screen::Map(_)))
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

    /// Re-roll the Home screensaver's contour pattern to `seed` (see
    /// [`HomeScreen::reseed`](crate::screen::HomeScreen::reseed)). The app does this itself when
    /// the stack returns to Home; this is the host-facing hook for previewing a specific pattern
    /// (the sim's `--home-seed`).
    pub fn reseed_home(&mut self, seed: u32) {
        if let Some(Screen::Home(home)) = self.stack.first_mut() {
            home.reseed(seed);
        }
    }

    /// Seed the live settings from the host's persistent store at boot. The host calls this
    /// once after construction with [`SettingsStore::load`](crate::hal::SettingsStore::load)'s
    /// value (or [`Settings::default`] when nothing is stored); it leaves the dirty flag clear,
    /// so seeding the boot value never triggers a needless write-back.
    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
        self.settings_dirty = false;
    }

    /// The live device settings — read by the host to persist them, and by anything that needs
    /// the current units / clock / GPS-interval outside the screen draw path.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Drain the "settings changed" flag set since the last call. The host checks this **once
    /// per frame** after [`handle_input`](App::handle_input) and, when set, persists
    /// [`settings`](App::settings) via its [`SettingsStore`](crate::hal::SettingsStore) — the
    /// settings analogue of [`take_dirty`](App::take_dirty)'s render demand. Idempotent: a
    /// frame with no settings edit returns `false` and writes nothing.
    pub fn take_settings_dirty(&mut self) -> bool {
        core::mem::take(&mut self.settings_dirty)
    }

    /// **Debug/benchmark hook** (the USB-CDC `Z` command): set the map camera to exactly `mpp`
    /// meters-per-pixel and force one map redraw. Drives the zoom directly — independent of the
    /// encoder's fixed 1.2× detents — so a host render sweep can pin an exact scale per sample,
    /// and always dirties the map (even at an unchanged scale) so each command yields one fresh
    /// frame to time. Part of the strippable render-instrumentation seam; no other caller.
    pub fn set_map_mpp(&mut self, mpp: f32) {
        self.state.zoom = zoom_for_mpp(mpp);
        self.map_dirty = true;
    }

    /// Recognise this frame's raw control input and apply each resulting gesture to the top
    /// screen, then advance the visible screens' timed content. The convenience that fuses the
    /// two planes into one call for the simulator and the firmware's single-executor fallback;
    /// `clock` is the [`InputClock`] (host/MCU wall-clock millis) for hold timing. Call once per
    /// frame even with no pending events — that is how a held button's long-press fires.
    ///
    /// The two-plane firmware does **not** call this: its high-priority plane owns a separate
    /// [`InputPlane`] that recognises gestures and feeds them back through
    /// [`apply_gesture`](App::apply_gesture), while [`advance_animations`](App::advance_animations)
    /// runs on the map plane. This method is exactly those two halves over `App`'s own
    /// [`InputPlane`], so all three hosts behave identically.
    pub fn handle_input(&mut self, clock: InputClock, input: &mut dyn InputSource) {
        self.now_ms = clock.0;
        // Recognise raw input into gestures and apply each. The borrow split is the point:
        // `recognize` borrows `self.input`, so the gestures are buffered there and applied
        // *after* it returns — `apply_gesture` touches the App's other fields, never
        // `self.input`. Recognition depends only on the raw events + the clock, so this is
        // identical to applying each gesture inline (capacity dwarfs one frame's events —
        // the input queue is bounded; overflow is unreachable, like `ButtonInput`'s queue).
        let mut pending: heapless::Vec<Gesture, GESTURE_BUF> = heapless::Vec::new();
        self.input.recognize(clock, input, |g| {
            let _ = pending.push(g);
        });
        for g in pending {
            self.apply_gesture(g);
        }
        self.advance_animations(clock);
    }

    /// Apply one recognised gesture to the top screen and run the navigation transition it
    /// returns — the **map plane's** half of input handling, split out from recognition.
    ///
    /// The two-plane firmware drains the high-priority plane's gesture channel and calls this
    /// for each gesture, in order, so the screen transition lands a frame after the overlay
    /// already confirmed the press — a clean flow rather than a frozen UI. Uses the map plane's
    /// clock ([`now_ms`](App::now_ms), set by [`advance_animations`](App::advance_animations) /
    /// [`handle_input`](App::handle_input)) for the [`Ctx`](screen::Ctx).
    pub fn apply_gesture(&mut self, g: Gesture) {
        // Any recognized gesture drives the top screen, and every screen — the map, the
        // menus, the Ride-control overlay — renders into the map plane (Layer 1), so an
        // applied gesture dirties it. Conservative by design (a gesture a screen ignores
        // still costs one redraw), which is what keeps the idle path exact: no gesture is
        // recognized, so `apply_gesture` never runs and the map stays clean — zero idle renders.
        self.map_dirty = true;
        // Snapshot the settings so a settings-screen edit is detected by one `==` (Settings is
        // `Copy + Eq`) — the same before/after trick `tick` uses on the camera `AppState`. A
        // change flags a save for the host to pick up via `take_settings_dirty`.
        let settings_before = self.settings;
        let App { state, activity, settings, catalog, stack, now_ms, .. } = self;
        let mut cx = Ctx { state, activity, settings, routes: catalog.as_slice(), now_ms: *now_ms };
        let t = stack.last_mut().expect("the stack always has the Home root").handle(g, &mut cx);
        let depth_before = stack.len();
        screen::apply(stack, t);
        // Returning to the bare Home root (the stack shrank back to just `[Home]`) re-opens the
        // screensaver — re-roll its contour seed so the topo peaks drift a little for this visit.
        // Gated on the *edge* (was deeper, now 1) so it fires once per return, never on a Home
        // gesture that stays on Home; and it's in `apply_gesture`, so a clock/battery re-render
        // (which never touches the stack) leaves the pattern put — the behaviour the look wants.
        if stack.len() == 1 && depth_before > 1 {
            if let Some(Screen::Home(home)) = stack.first_mut() {
                home.reseed(*now_ms);
            }
        }
        if self.settings != settings_before {
            self.settings_dirty = true;
        }
    }

    /// Advance the **map plane's** clock to `clock` and let each visible screen surface any
    /// time-driven repaint need — today the Statistics cursor's spring-back to the live
    /// position — dirtying the map if any advanced. So a screen surfaces its own timed-refresh
    /// rather than the host re-rendering on a blind heartbeat (issue #47). Cheap: a clock
    /// comparison per drawn screen, over the same `base..` range [`render_map`](App::render_map)
    /// draws (so an overlay over a riding view still lets the view underneath settle).
    ///
    /// [`handle_input`](App::handle_input) calls this for the single-loop hosts; the two-plane
    /// firmware calls it directly on its map plane (its input lives on a separate executor).
    pub fn advance_animations(&mut self, clock: InputClock) {
        self.now_ms = clock.0;
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        let mut animated = false;
        for scr in self.stack.iter_mut().skip(base) {
            animated |= scr.animate(self.now_ms);
        }
        self.map_dirty |= animated;
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
    /// into the *same* target, in that order. Hosts that keep the map and overlay on
    /// separate buffers/layers (dual-layer display) call the two halves directly instead.
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
        // Untimed: the host's `NoopClock` leaves the map's per-stage `*_us` fields at 0. The
        // device uses `render_map_timed` with a real clock for the render benchmark. This
        // convenience entry is always drawing the map, so it passes `Some`.
        self.render_map_timed(target, Some(reader), route, w, h, color_fn, &NoopClock)
    }

    /// Like [`render_map`](App::render_map) but threads `clock` to the Map screen's
    /// [`render_timed`](obc_render::MapRenderer::render_timed), so the returned [`RenderStats`]
    /// carries the map's per-stage timings (`collect_us` / `sort_us` / `draw_us`). The device's
    /// render benchmark uses this with its own microsecond clock; every other host calls the plain
    /// [`render_map`](App::render_map). Part of the strippable render-instrumentation seam.
    #[allow(clippy::too_many_arguments)]
    pub fn render_map_timed<D, F>(
        &mut self,
        target: &mut D,
        reader: Option<&Reader>,
        route: Option<&RouteReader>,
        w: f32,
        h: f32,
        color_fn: F,
        clock: &dyn Clock,
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
        // The in-screen confirm fill (RideControl / RouteSwap) tracks the encoder hold-progress
        // owned by the input plane. On the single-loop hosts that is `App`'s own, kept live by
        // `handle_input`. On the two-plane firmware `App` owns only the map plane and this reads
        // `0.0` — which matches the render-on-demand behaviour anyway: a pure hold-charge never
        // dirties the map (issue #47), so the map (and this fill) doesn't redraw mid-charge; the
        // live confirmation is the overlay bulge on its own high-priority plane.
        // Prefer a host-supplied hold-progress (the two-plane firmware's separate input plane); fall
        // back to `App`'s own input on the single-loop hosts (the sim) that never set the override.
        let hold_progress = self.hold_progress_override.unwrap_or_else(|| self.input.encoder_hold_progress());
        let App { state, activity, settings, catalog, renderer, stack, now_ms, profile, breadcrumb, .. } = self;
        let mut rx = Render {
            reader,
            renderer,
            state,
            activity,
            settings,
            routes: catalog.as_slice(),
            route,
            profile: profile.as_ref(),
            breadcrumb: &*breadcrumb,
            w,
            h,
            now_ms: *now_ms,
            hold_progress,
            clock,
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
        self.input.render_overlay(target, w, h, color_fn);
    }

    /// Whether the overlay plane has live content this frame — a hold bulge that is
    /// charging, popping, or retracting. A host driving the map and overlay as
    /// separate layers polls this to decide when [`render_overlay`](App::render_overlay)
    /// must repaint over an unchanged map; it is `false` exactly when the overlay
    /// would draw nothing, so the overlay layer can stay idle.
    pub fn overlay_active(&self) -> bool {
        self.input.overlay_active()
    }

    /// Drain the repaint demand accumulated since the last call, resetting to
    /// [`Dirty::CLEAN`]. The host calls this **once per frame** after [`tick`](App::tick) +
    /// [`handle_input`](App::handle_input), then renders [`render_map`](App::render_map) only
    /// when [`Dirty::map`] and [`render_overlay`](App::render_overlay) only when
    /// [`Dirty::overlay`] — the render-on-demand loop (issue #47).
    ///
    /// [`map`](Dirty::map) is the accumulation of every map-affecting mutation since the last
    /// drain (an applied gesture, a camera-moving fix on a riding view, a route/session change,
    /// a screen's timed `animate`). [`overlay`](Dirty::overlay) is *derived* by the
    /// [`InputPlane`] from the live hold-bulge state rather than accumulated: it's set while
    /// the bulge is live, plus exactly one trailing frame after it goes quiet so the host can
    /// clear it off Layer 2. Because that trailing edge is tracked across calls, draining twice
    /// in one frame would swallow it — call exactly once per frame.
    ///
    /// (The two-plane firmware doesn't use the `overlay` flag here — its high-priority plane
    /// drives the overlay from its *own* [`InputPlane::take_overlay_dirty`]; this `App` owns
    /// only the map plane there. The single-loop hosts use both.)
    pub fn take_dirty(&mut self) -> Dirty {
        Dirty { map: core::mem::take(&mut self.map_dirty), overlay: self.input.take_overlay_dirty() }
    }

    /// The most recently recognized gesture (host input readout), if any.
    pub fn last_gesture(&self) -> Option<Gesture> {
        self.input.last_gesture()
    }

    /// In-flight encoder hold-progress (0.0–1.0) for the confirm-ring readout.
    pub fn encoder_hold_progress(&self) -> f32 {
        self.input.encoder_hold_progress()
    }

    /// Feed the live encoder hold-progress (0.0–1.0) used by the in-screen confirm fills (the
    /// factory Reset bar). The **two-plane firmware** calls this each frame from its high-priority
    /// [`InputPlane`], whose hold state `App`'s own plane doesn't see — without it the Reset bar
    /// never fills on the device. The single-loop hosts (the sim) never call it; the render then
    /// reads `App`'s own input. Pairs with [`base_draws_map`](App::base_draws_map): the host
    /// forces a redraw while a hold charges on a cheap (non-map) screen so the fill actually
    /// animates (a pure hold-charge doesn't otherwise dirty the map — issue #47).
    pub fn set_hold_progress(&mut self, progress: f32) {
        self.hold_progress_override = Some(progress);
    }

    /// In-flight Back hold-progress (0.0–1.0).
    pub fn back_hold_progress(&self) -> f32 {
        self.input.back_hold_progress()
    }

    /// The current operating mode.
    pub fn mode(&self) -> Mode {
        self.activity.mode
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hal::{CompassSource, LocationSource};

    /// A location source that yields one fix then runs dry (so a single `tick` integrates it).
    struct OneFix(Option<Fix>);
    impl LocationSource for OneFix {
        fn poll(&mut self) -> Option<Fix> {
            self.0.take()
        }
    }

    /// The Home root's current backdrop seed.
    fn home_seed(app: &App) -> u32 {
        match app.stack.first() {
            Some(Screen::Home(h)) => h.seed(),
            _ => panic!("Home is always the stack root"),
        }
    }

    /// The backdrop re-rolls when the stack *returns* to the bare Home root — once per return, on
    /// the edge — and stays put for any gesture that doesn't reach Home. (A clock/battery re-render
    /// goes through `tick`/render, never `apply_gesture`, so by construction it can't reseed.)
    #[test]
    fn returning_to_home_rerolls_the_backdrop_seed() {
        let mut app = App::new_idle(AppState::new(0, 0, 0.05)); // [Home], the canonical seed
        assert_eq!(home_seed(&app), 0, "boot starts on the un-jittered massif");

        app.now_ms = 4242;
        app.apply_gesture(Gesture::BackHold); // Home → Menu (stack grows)
        assert_eq!(home_seed(&app), 0, "going deeper than Home does not reseed");

        app.apply_gesture(Gesture::Back); // Menu → Pop → back to [Home]
        assert_eq!(home_seed(&app), 4242, "returning to Home re-rolls from the wall clock");

        // A gesture Home ignores leaves the stack — and so the pattern — untouched.
        app.now_ms = 9999;
        app.apply_gesture(Gesture::Turn(1));
        assert_eq!(home_seed(&app), 4242, "a no-op gesture on Home keeps the same pattern");
    }

    /// A compass that always reports the same heading.
    struct ConstCompass(f32);
    impl CompassSource for ConstCompass {
        fn poll(&mut self) -> Option<f32> {
            Some(self.0)
        }
    }

    /// An altimeter that yields one altitude sample then runs dry (so a single `tick`
    /// integrates exactly one barometric reading, matching the once-per-tick contract).
    struct OneAlt(Option<f32>);
    impl crate::hal::AltimeterSource for OneAlt {
        fn poll(&mut self) -> Option<f32> {
            self.0.take()
        }
    }

    fn moving(course: f32) -> Fix {
        Fix { lat: 0, lon: 0, course: Some(course), speed_mps: Some(5.0) }
    }

    // --- the heading fallback chain (course_rad / live_course_rad) ---

    #[test]
    fn heading_up_uses_gps_course_when_moving() {
        let mut s = AppState::new(0, 0, 1.0);
        s.heading_up = true;
        s.user_fix = Some(moving(90.0));
        s.compass_deg = Some(180.0); // ignored: the GPS has a course
        assert!((s.course_rad() - 90f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn heading_up_falls_back_to_compass_when_stopped() {
        let mut s = AppState::new(0, 0, 1.0);
        s.heading_up = true;
        s.user_fix = Some(Fix::at(0, 0)); // stationary → no course
        s.compass_deg = Some(270.0);
        assert!((s.course_rad() - 270f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn north_up_ignores_compass() {
        let mut s = AppState::new(0, 0, 1.0);
        s.heading_up = false;
        s.user_fix = Some(Fix::at(0, 0));
        s.compass_deg = Some(123.0);
        assert_eq!(s.course_rad(), 0.0, "north-up holds north regardless of the compass");
    }

    #[test]
    fn stopped_without_compass_holds_north() {
        let mut s = AppState::new(0, 0, 1.0);
        s.heading_up = true;
        s.user_fix = Some(Fix::at(0, 0));
        assert_eq!(s.course_rad(), 0.0);
    }

    // --- tick adoption gating (don't store the compass where it would force a redraw) ---

    fn tick_with(app: &mut App, fix: Fix, compass_deg: f32) {
        let mut loc = OneFix(Some(fix));
        let mut compass = ConstCompass(compass_deg);
        app.tick(
            RideClock(1000),
            Sensors { loc: &mut loc, altimeter: None, compass: Some(&mut compass), track: None, fuel: None },
            None,
        );
    }

    #[test]
    fn tick_adopts_compass_when_stopped_and_heading_up() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.state.heading_up = true;
        tick_with(&mut app, Fix::at(0, 0), 200.0);
        assert_eq!(app.state.compass_deg, Some(200.0));
    }

    #[test]
    fn tick_ignores_compass_while_moving() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.state.heading_up = true;
        tick_with(&mut app, moving(45.0), 200.0);
        assert_eq!(app.state.compass_deg, None, "GPS course wins → compass not stored while moving");
    }

    #[test]
    fn tick_ignores_compass_when_north_up() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.state.heading_up = false; // north-up never consults the compass
        tick_with(&mut app, Fix::at(0, 0), 200.0);
        assert_eq!(app.state.compass_deg, None);
    }

    // --- in-place placement into the reserved region (issue #67) ---

    /// `init_idle` writing field-by-field into a slot must land the same power-on state
    /// `new_idle` builds by value — Home root, Idle, nothing loaded, map dirty — with the
    /// renderer zeroed in place. Guards against a field being forgotten in the in-place path.
    #[test]
    fn init_idle_matches_new_idle() {
        use core::mem::MaybeUninit;
        let state = AppState::new(1, 2, 3.0);
        // Placement target. ~200 KB on the host test stack is fine; the point being exercised
        // is that no *second* `App`-sized temporary is formed (init_idle writes straight in).
        let mut slot = MaybeUninit::<App>::uninit();
        // SAFETY: `slot` is a valid, aligned, exclusively-owned `App` region; init_idle fully
        // initializes it before assume_init_ref reads it.
        let placed = unsafe {
            App::init_idle(slot.as_mut_ptr(), state);
            slot.assume_init_ref()
        };

        assert_eq!(placed.state, state, "camera state is preserved verbatim");
        assert_eq!(placed.activity.mode, Mode::Idle, "boots Idle, not Riding");
        assert!(placed.map_dirty, "first frame must paint");
        assert_eq!(placed.now_ms, 0);
        assert!(placed.profile.is_none() && placed.profile_route.is_none());
        assert!(placed.matched_route.is_none() && placed.ride_session.is_none());
        assert!(placed.breadcrumb.is_empty(), "no breadcrumb before any ride");
        // The stack is exactly the Home root, like `new_idle`.
        let reference = App::new_idle(state);
        assert_eq!(placed.stack.len(), reference.stack.len());
        assert_eq!(placed.stack.len(), 1);
        assert!(matches!(placed.stack[0], Screen::Home(_)), "Home is the stack root");
    }

    // --- end-to-end barometric climb through `tick` (issue #93 item 1) ---

    /// Feed one altitude sample through `App::tick`'s `Sensors.altimeter` arm (app.rs ~513). No
    /// previous test ever attached an altimeter, so the wiring from `tick` → `record_altitude` →
    /// `climb_m` was untested end-to-end. This walks a real climb in tick-sized steps and reads
    /// the `climbed` stat through the public `App`, proving the barometer actually drives it.
    fn tick_alt(app: &mut App, alt_m: f32, now_ms: u32) {
        let mut loc = OneFix(None); // no fix this tick — isolate the altimeter path
        let mut alt = OneAlt(Some(alt_m));
        app.tick(
            RideClock(now_ms),
            Sensors { loc: &mut loc, altimeter: Some(&mut alt), compass: None, track: None, fuel: None },
            None,
        );
    }

    #[test]
    fn tick_integrates_barometric_climb_dead_banded() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // boots Riding
        tick_alt(&mut app, 100.0, 1000); // anchor
        assert_eq!(app.activity.climb_m(), 0.0, "the first sample only anchors");
        tick_alt(&mut app, 102.0, 2000); // +2 m: inside the dead-band
        assert_eq!(app.activity.climb_m(), 0.0, "sub-dead-band noise books nothing through tick");
        tick_alt(&mut app, 110.0, 3000); // +10 m from the 100 m reference
        assert_eq!(app.activity.climb_m(), 10.0, "a clean climb books through the full tick path");
    }

    /// The pause rule end-to-end: with the activity paused (the Ride-control state), `tick` still
    /// records the latest altitude but must not book climb across the rest — so a barometer drift
    /// while stopped doesn't inflate `climbed` when riding resumes. Mirrors the unit test in
    /// `activity.rs`, but proves the *whole tick path* honours the mode gate (app.rs ~513-516).
    #[test]
    fn tick_does_not_book_climb_while_paused() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        tick_alt(&mut app, 100.0, 1000); // anchor while riding
        tick_alt(&mut app, 110.0, 2000); // +10 m climbed
        assert_eq!(app.activity.climb_m(), 10.0);

        app.activity.mode = Mode::Paused; // as a `press` → Ride control would set it
        tick_alt(&mut app, 160.0, 3000); // +50 m of drift during the stop
        tick_alt(&mut app, 160.0, 4000);
        assert_eq!(app.activity.climb_m(), 10.0, "no climb accrues across a paused tick");

        app.activity.mode = Mode::Riding; // resume
        tick_alt(&mut app, 160.0, 5000); // re-anchors at the current height
        tick_alt(&mut app, 165.0, 6000); // a real +5 m after resuming
        assert_eq!(app.activity.climb_m(), 15.0, "only genuine post-resume climb adds through tick");
    }

    // --- settings persistence signal (the host's save trigger) ---

    /// A settings edit applied through `apply_gesture` flags `take_settings_dirty` (so the host
    /// persists it); a gesture that changes no setting does not. Drives the real navigation
    /// Home → Menu → Settings → Units, so the whole `Ctx { settings }` plumbing is exercised.
    #[test]
    fn a_settings_edit_flags_dirty_and_a_noop_does_not() {
        use crate::settings::Units;
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        // Walk to the Units screen (Menu = Routes/Settings; Settings list = Date&Time/Units/…).
        app.apply_gesture(Gesture::BackHold); // Home → Menu
        app.apply_gesture(Gesture::Turn(1)); // → Settings row
        app.apply_gesture(Gesture::Press); // → Settings list
        app.apply_gesture(Gesture::Turn(1)); // → Units row
        app.apply_gesture(Gesture::Press); // → Units screen
        assert!(!app.take_settings_dirty(), "navigation changed no setting, so nothing to save");

        let before = app.settings().units;
        app.apply_gesture(Gesture::Press); // flip units
        assert_ne!(app.settings().units, before, "the Units screen flipped the system");
        assert_eq!(app.settings().units, Units::Imperial, "default Metric → Imperial");
        assert!(app.take_settings_dirty(), "a real settings change flags a save");
        assert!(!app.take_settings_dirty(), "and the flag drains — only saved once");

        app.apply_gesture(Gesture::Back); // leave Units (no settings change)
        assert!(!app.take_settings_dirty(), "a non-settings gesture leaves the flag clear");
    }

    /// `set_settings` seeds the boot value without arming a save (the value came from the store /
    /// the default — re-persisting it would be a pointless write).
    #[test]
    fn set_settings_does_not_flag_dirty() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let seeded = crate::settings::Settings { units: crate::settings::Units::Imperial, ..Default::default() };
        app.set_settings(seeded);
        assert_eq!(app.settings().units, crate::settings::Units::Imperial);
        assert!(!app.take_settings_dirty(), "seeding the boot value must not trigger a write-back");
    }
}
