//! [`AppState`] — the device's view state — and [`App`], the shared per-frame
//! driver that both hosts run.

use embedded_graphics::{draw_target::DrawTarget, primitives::Rectangle};
use obc_reader::Reader;
use obc_render::{zoom_for_mpp, Canvas, Clock, MapRenderer, NoopClock, RenderStats, Viewport};
use obc_route::{Profile, RouteMatch, RouteReader, TrackPoint};

use crate::activity::{Activity, Mode};
use crate::breadcrumb::Breadcrumb;
use crate::dirty::Dirty;
use crate::hal::{Fix, InputClock, InputSource, LocationSource, RideClock, Sensors};
use crate::input::Gesture;
use crate::input_plane::InputPlane;
use crate::ride::RideSummary;
use crate::route::{Catalog, RouteSummary};
use crate::screen::{self, Ctx, HomeScreen, MapScreen, Render, Screen, Stack, WarningFlags};
use crate::settings::{DateTime, Settings};
use crate::wall_clock::WallClock;

/// How the camera relates to the user's position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    /// The camera tracks the user — every fix recenters the map on it. The normal navigation mode.
    Follow,
    /// The camera is driven manually (the simulator's mouse pan/zoom) and ignores the user's
    /// position; fixes are still recorded for the marker.
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

/// The device's view state: where the camera looks, how zoomed in it is, what mode it's in, and
/// the last known user fix.
///
/// The shared core the host renders. The host owns the display size and the
/// [`obc_render::MapRenderer`]/draw target; each frame it calls [`update`] with the platform's
/// [`LocationSource`], then [`viewport`] for the camera to render through. The split keeps display
/// dimensions out of the shared state.
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
    /// Battery charge, 0–100 %. [`App::tick`] writes it from the [`FuelGauge`](crate::FuelGauge);
    /// the Home screen draws the gauge from it (filled bars coloured by level, empty bars dim grey).
    pub battery_pct: u8,
    /// The BLE link phase (Off / Advertising / Connected). [`App::set_ble_status`] writes it from
    /// the host's [`BleStatus`](crate::BleStatus); the connected indicator (the menu title bar's
    /// right slot and the Home battery row) draws on [`Connected`](crate::BleLink::Connected) only
    /// — see [`ble_connected`](AppState::ble_connected) — while the Bluetooth settings screen's
    /// status line shows all three states. It lives **on** `AppState` — unlike `temp_c` — precisely
    /// because drawn views react to it: a change is meant to gate a repaint, and the
    /// `state != state_before` comparison already routes that to the screen that draws it.
    pub ble_link: crate::BleLink,
    /// A BLE bond is stored (the board's RRAM bond slot / the sim's injected flag) — the Bluetooth
    /// screen's "Paired: yes/no" row. Fed by [`App::set_ble_status`] like [`ble_link`](AppState::ble_link).
    pub ble_paired: bool,
    /// The Bluetooth screen's **"Forget phone"** request (epic #447, P8): set by the screen's
    /// guarded hold, drained by the host via [`App::take_ble_forget`] — which clears the RRAM bond
    /// slot and drops the bonded connection on the board, or clears the injected `paired` flag in
    /// the sim. A pending app→host command, carried here because `AppState` is the one mutable
    /// app-wide state a screen's `handle` reaches (the `TrackAction` pattern, one plane over).
    pub ble_forget_pending: bool,
}

impl AppState {
    /// A fresh state centered at `(cam_lon, cam_lat)` microdegrees with the given `zoom`, in
    /// [`Follow`](CameraMode::Follow) mode and no fix yet.
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
            // Stand-in until a [`FuelGauge`](crate::FuelGauge) feeds a real reading on the first tick.
            battery_pct: 75,
            // No phone linked until the host feeds the first [`BleStatus`](crate::BleStatus).
            ble_link: crate::BleLink::Advertising,
            ble_paired: false,
            ble_forget_pending: false,
        }
    }

    /// Whether a phone holds the BLE link — the connected indicator's one question
    /// ([`ble_link`](AppState::ble_link) == [`Connected`](crate::BleLink::Connected)).
    pub fn ble_connected(&self) -> bool {
        self.ble_link == crate::BleLink::Connected
    }

    /// Advance one tick: poll the location source and, in [`Follow`](CameraMode::Follow) mode,
    /// recenter the camera on the new fix. In [`Free`](CameraMode::Free) mode the fix is still
    /// recorded (for the marker) but the camera stays where the host's pan/zoom put it. No fix this
    /// tick leaves everything untouched (a dropout holds the last camera position).
    ///
    /// Returns the new [`Fix`] when one arrived this tick, else `None`.
    pub fn update(&mut self, loc: &mut dyn LocationSource) -> Option<Fix> {
        let fix = loc.poll()?;
        self.user_fix = Some(fix);
        // Recenter only when following — guard on `pan` too so a frozen camera can't be yanked back
        // by an incoming fix.
        if self.mode == CameraMode::Follow && self.pan.is_none() {
            self.cam_lon = fix.lon;
            self.cam_lat = fix.lat;
        }
        Some(fix)
    }

    /// Project the current camera into a [`Viewport`] for a `w`×`h` pixel display. In
    /// [`heading_up`](AppState::heading_up) mode the projection rotates so the last fix's `course`
    /// points to the top of the screen; with no course (or north-up) it stays north-up.
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
        self.effective_heading_deg().map_or(0.0, |deg| deg.to_radians())
    }

    /// The rider's heading reference in degrees CW from north, or `None` when neither is known —
    /// the GPS [`course`](Fix::course) while moving, else the electronic [`compass_deg`] while
    /// stopped (the #231 seam). Unlike [`live_course_rad`](AppState::live_course_rad) this doesn't
    /// fall back to north: a consumer that must *hide* rather than mislead (the POI list's
    /// bearing arrows) keys off the `None`. The heading-up map's rotation folds this `None` to
    /// north through `live_course_rad`, so the arrow and the map agree whenever a heading exists.
    pub fn effective_heading_deg(&self) -> Option<f32> {
        self.user_fix.and_then(|f| f.course).or(self.compass_deg)
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

/// Camera travel **per encoder detent** in pan mode, in screen pixels — a *screen* amount (not
/// ground metres), so panning is finer when zoomed in.
pub const PAN_STEP_PX: f32 = 40.0;

/// Capacity of [`handle_input`](App::handle_input)'s per-frame gesture buffer. One frame yields at
/// most one gesture per raw event (the input queue is bounded — `ButtonInput`'s is 8) plus the
/// single per-frame long-press, so this never overflows.
const GESTURE_BUF: usize = 16;

/// The whole device application, ready to run a frame.
///
/// The single entry point both hosts share: each constructs one `App`, then per frame
/// [`tick`](App::tick)s it with their [`LocationSource`], feeds raw controls through
/// [`handle_input`](App::handle_input), and [`render_frame`](App::render_frame)s to their display.
/// `App` owns the screen stack, the input + overlay plane ([`InputPlane`]), the camera
/// [`AppState`], the ride [`Activity`], and the reusable [`MapRenderer`].
///
/// The firmware can split the two planes across executors — recognising gestures on a
/// high-priority [`InputPlane`] that preempts the map render and feeding them back through
/// [`apply_gesture`](App::apply_gesture); [`handle_input`](App::handle_input) is those halves fused
/// for the single-loop hosts.
///
/// ```ignore
/// let mut app = App::new(AppState::new(cx, cy, zoom));
/// loop {
///     // GPS + barometer + compass + active route → camera, map-match, ride stats.
///     let sensors = Sensors {
///         loc: &mut location_source,
///         altimeter: Some(&mut baro),
///         temperature: Some(&mut thermometer),
///         clock: Some(&mut gps_clock),
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
    /// The camera / orientation / last-fix state — public so the host's mouse pan/zoom and control
    /// panel can read and adjust it directly.
    pub state: AppState,
    /// The ride mode + tracking accumulators.
    pub activity: Activity,
    /// The resident route catalog (summaries), populated by the host ([`set_routes`](App::set_routes)).
    /// The Route menu lists it; `active_route` indexes it.
    catalog: Catalog,
    /// Each catalog entry's **durable object id**, parallel to [`catalog`](App::catalog) (#450).
    /// The identity that survives a live rescan: on every [`set_routes_with_ids`](App::set_routes_with_ids)
    /// every held catalog *index* (`active_route`, an open Route-menu selection, a pending swap) is
    /// remapped old-id → new-index, so an inserted/removed route can never silently shift which
    /// route is navigated. The firmware feeds the filename-encoded upload ids (+ session-scoped
    /// side-load ids); hosts without ids get positional ones from [`set_routes`](App::set_routes).
    catalog_ids: heapless::Vec<u16, { crate::route::MAX_ROUTES }>,
    /// The resident ride catalog (summaries), populated by the host ([`set_rides`](App::set_rides)) —
    /// the Rides screen lists it (epic #447, P7). Each entry carries its `synced` flag; the parallel
    /// [`ride_catalog_ids`](App::ride_catalog_ids) holds the durable object ids the hold-to-delete
    /// footer resolves against.
    ride_catalog: crate::ride::RideCatalog,
    /// Each ride-catalog entry's **durable object id**, parallel to [`ride_catalog`](App::ride_catalog)
    /// — the identity the Rides-menu selection follows across a live rescan, and what the
    /// hold-to-delete drain resolves a highlighted index to.
    ride_catalog_ids: heapless::Vec<u16, { crate::ride::UI_RIDES_CAP }>,
    /// The screen stack (root = Home). The top screen receives input; drawing starts from the
    /// topmost opaque screen so overlays composite over the map.
    stack: Stack,
    /// The active route's resident elevation profile, rebuilt on route load (it streams every
    /// chunk, so never per frame). `None` when no route is loaded;
    /// [`profile_route`](App::profile_route) tracks which route it was built for.
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
    /// Reused renderer; clears (not frees) its scratch each frame, so steady-state rendering does no
    /// allocation — important on the MCU.
    renderer: MapRenderer,
    /// The input + overlay plane: gesture recognizer, long-press hint overlay, live hold-progress.
    /// Split off `App` so the firmware can run it on a *separate, high-priority* executor that
    /// preempts the map render. `App` keeps this one for the [`handle_input`](App::handle_input)
    /// path; the two-plane firmware drives its own and feeds gestures back through
    /// [`apply_gesture`](App::apply_gesture).
    input: InputPlane,
    /// Millis at the last [`handle_input`](App::handle_input) /
    /// [`advance_animations`](App::advance_animations) — the **map plane's** clock, distinct from
    /// the input plane's own clock.
    now_ms: u32,
    /// Accumulated **map-plane** repaint demand since the last [`take_dirty`](App::take_dirty),
    /// drained once per frame. Starts `true` so the host's first frame paints. (The overlay flag
    /// isn't accumulated here — it's derived from the live hold-bulge state at drain time.)
    map_dirty: bool,
    /// Accumulated **region-scoped** repaint demand (#500 follow-up): the union of every
    /// region-carrying screen-tick change since the last drain — the nav-planning spinner's
    /// needle disc. Kept apart from [`map_dirty`](App::map_dirty) so the two can't blur: any
    /// full-frame demand (every other `map_dirty = true` site) overrides this at
    /// [`take_dirty`](App::take_dirty), and region ticks never set `map_dirty` — see the drain
    /// for the fold.
    region_dirty: Option<Rectangle>,
    /// Panel size (device px) of the last rendered frame, recorded by
    /// [`render_map_timed`](App::render_map_timed) — what
    /// [`advance_animations`](App::advance_animations) hands the screen ticks so a reported
    /// [`ScreenTick::region`](screen::ScreenTick::region) is sized to the real panel. `(0, 0)`
    /// until the first frame; region reporting abstains (full repaint) until then.
    frame_size: (i32, i32),
    /// One-shot clip for the **next** [`render_map_timed`](App::render_map_timed): the host that
    /// drained a region-scoped [`Dirty`](crate::Dirty) sets it via
    /// [`set_render_clip`](App::set_render_clip) so the frame's `Canvas` rejects whole primitives
    /// outside the region — the draw-call machinery (glyph decode, scanline iterators) a
    /// pixel-level framebuffer clip can't skip. Taken (cleared) by the render, so a host that
    /// never sets it — the sim, the tests — always draws full frames.
    render_clip: Option<Rectangle>,
    /// The soonest timed-redraw deadline across the visible stack, in millis from the last
    /// [`advance_animations`](App::advance_animations) — the min-fold of each screen's
    /// [`ScreenTick::next_wake_ms`](screen::ScreenTick::next_wake_ms), stored there and read back by
    /// [`ms_until_next_wake`](App::ms_until_next_wake). `None` when nothing is time-animating.
    next_wake_ms: Option<u32>,
    /// The persisted device settings, seeded from the host's store at boot
    /// ([`set_settings`](App::set_settings)) and edited in place by the settings screens.
    settings: Settings,
    /// The live wall clock: [`settings.clock`](Settings::clock) (a set-point) advanced by elapsed
    /// monotonic millis — there's no RTC, so this is how a static readout ticks. Re-stamped whenever
    /// the set-point changes in [`set_settings`](App::set_settings) /
    /// [`apply_gesture`](App::apply_gesture). See [`WallClock`].
    wall_clock: WallClock,
    /// Whether a [`settings`](App::settings) edit is **pending persistence**; set by
    /// [`apply_gesture`](App::apply_gesture)'s before/after compare, drained by
    /// [`take_settings_dirty`] once the user leaves the settings subtree (the save is debounced to
    /// screen exit, not fired per detent). Starts `false` — the boot value came from the store or
    /// the default.
    ///
    /// [`take_settings_dirty`]: App::take_settings_dirty
    settings_dirty: bool,
    /// Host-supplied encoder hold-progress (0.0–1.0) for the in-screen confirm fills (the factory
    /// Reset bar; [`RideControl`](crate::screen::RideControl) confirm rows). `None` on the
    /// single-loop hosts (the render reads `App`'s own [`InputPlane`]); the **two-plane firmware**
    /// feeds live progress in each frame via [`set_hold_progress`](App::set_hold_progress), since
    /// its holds live on a separate plane `App`'s own never sees.
    hold_progress_override: Option<f32>,
    /// Set by [`apply_gesture`](App::apply_gesture) whenever a gesture **changed the screen
    /// stack**: any hold charging at that moment was aimed at a screen that is no longer the
    /// top, so it must be cancelled rather than delivered to whatever replaced it (a hold aimed
    /// at a popup's "Save & new" must never land on the Route menu's hold-to-delete footer —
    /// issue #480). [`handle_input`](App::handle_input) drains it inline (cancelling `input`'s
    /// holds and dropping stray `Hold`/`BackHold`s later in the same batch); the two-plane
    /// firmware drains it via [`take_hold_cancel`](App::take_hold_cancel) and cancels its own
    /// input plane's recogniser.
    hold_cancel_pending: bool,
    /// Millis of the last battery [`FuelGauge`](crate::FuelGauge) poll, or `None` before the first.
    /// Read on a slow cadence ([`BATTERY_POLL_MS`]) — *not* every tick — so a real PMIC read never
    /// spins the I²C bus at the frame rate.
    last_battery_poll_ms: Option<u32>,
    /// Last ambient temperature (°C), or `None` before the first sample / no thermometer. Held
    /// across ticks. No screen consumes it yet, so it lives **off** [`AppState`] — storing it there
    /// would gate a needless map redraw on every reading, breaking render-on-demand. Read via
    /// [`temperature_c`](App::temperature_c).
    temp_c: Option<f32>,
    /// Map-plane millis of the last accepted GPS fix, or `None` before the first ever. Drives the
    /// "No GPS Fix" banner via [`has_live_fix`](App::has_live_fix). Lives **off** [`AppState`] —
    /// like [`temp_c`](App::temp_c) — so advancing it on every fix (incl. a stationary one) never
    /// trips the `state != state_before` redraw gate; the banner's own repaint edge comes from
    /// [`advance_animations`](App::advance_animations) instead.
    last_fix_ms: Option<u32>,
    /// The no-fix state at the previous [`advance_animations`](App::advance_animations), so the
    /// timer edge that flips the "No GPS Fix" banner dirties the live-data views exactly once.
    /// Starts `true` — no fix at boot.
    prev_no_fix: bool,
    /// The single POI-list snapshot buffer (issue #425), threaded into the draw context as
    /// [`Render::poi_scratch`]. Held once here rather than per-screen so the ~800 B doesn't multiply
    /// across the screen-stack union (see [`PoiScratch`](crate::screen::PoiScratch)). Filled lazily
    /// by the POI list screen's first draw; invalidated in [`apply_gesture`](App::apply_gesture)
    /// when a POI list opens, so re-entering a category re-queries.
    poi_scratch: screen::PoiScratch,
    /// The live BLE pairing passkey ([`BleStatus::passkey`](crate::BleStatus)), fed by
    /// [`set_ble_status`](App::set_ble_status) and driving the passkey card (P2, #449) via
    /// [`reconcile_passkey_card`](App::reconcile_passkey_card). Held off `AppState` so feeding it
    /// never gates a map redraw; [`ble_passkey`](App::ble_passkey) exposes it for tests to observe
    /// the seam carrying it.
    ble_passkey: Option<u32>,
    /// Count of [`notify_store_changed`](App::notify_store_changed) calls not yet acted on. The host
    /// drains it once per pass via [`take_store_changed`](App::take_store_changed) and answers a
    /// non-zero count with a store rescan → [`set_routes_with_ids`](App::set_routes_with_ids) (#450).
    /// A counter, not a bool, so a burst of commits between drains is never coalesced into a single
    /// missed rescan.
    store_changed_pending: u32,
    /// The one **pending route-upload prompt** (epic #447, P4), set by
    /// [`notify_route_uploaded`](App::notify_route_uploaded) and delivered (or dropped) by
    /// [`reconcile_upload_prompt`](App::reconcile_upload_prompt). Deliberately a single slot:
    /// consecutive uploads replace it — most recent wins, the popup rule. Carried by **durable
    /// object id**, never a catalog index, so a rescan between arrival and a hold-deferred
    /// delivery can't retarget it.
    pending_upload: Option<UploadEvent>,
    /// Device warnings **discovered but not yet shown** on the advisory card (issue #504) — a
    /// missing-sensor probe result, or the map-slow flag. Accumulated by
    /// [`notify_warning`](App::notify_warning) and delivered (or deferred behind a passkey card /
    /// hold) by [`reconcile_warning`](App::reconcile_warning), like [`pending_upload`].
    pending_warnings: WarningFlags,
    /// Warnings **already shown** on a card this session, so each flag is surfaced once and a
    /// dismissed notice doesn't nag — while a genuinely *new* flag (e.g. a late sensor timeout)
    /// still re-opens the card. Never cleared (the boot's warnings are the boot's).
    warned: WarningFlags,
}

/// One committed route upload, as [`notify_route_uploaded`](App::notify_route_uploaded) queues it
/// for prompt delivery (epic #447, P4).
#[derive(Debug, Clone, Copy)]
struct UploadEvent {
    /// The committed route's durable object id — resolved to a catalog index at *delivery* time.
    id: u16,
    /// The upload replaced the **actively-navigated** route (snapshotted at arrival): the
    /// info-only "ROUTE UPDATED" card instead of a choice prompt — adoption already happened.
    active_replace: bool,
}

/// A fix older than this (map-plane millis) means "no current GPS fix". The window is the larger of
/// this floor and a few fix intervals (see [`App::no_fix_window_ms`]), so a long configured
/// interval doesn't false-trip the banner between its own expected fixes.
const NO_FIX_FLOOR_MS: u32 = 5_000;
/// How many configured fix intervals of silence count as "lost" before the floor takes over.
const NO_FIX_INTERVALS: u32 = 3;

/// How often [`App::tick`] reads the battery [`FuelGauge`](crate::FuelGauge). Charge drifts over
/// minutes, so a ~30 s cadence keeps the Home gauge fresh while reading the PMIC a few times a
/// minute at most. Independent of redraws: an unchanged reading repaints nothing.
const BATTERY_POLL_MS: u32 = 30_000;

impl App {
    /// Build the app straight onto the live map: stack `[Home, Map]`, Home the always-present root
    /// that Finish / Discard return to, no route loaded. The map-first constructor the simulator
    /// uses for headless `--png` renders (and the tests); the GUI and device boot via
    /// [`new_idle`](App::new_idle).
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
            catalog_ids: heapless::Vec::new(),
            ride_catalog: crate::ride::RideCatalog::new(),
            ride_catalog_ids: heapless::Vec::new(),
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
            region_dirty: None,
            frame_size: (0, 0),
            render_clip: None,
            next_wake_ms: None,
            settings: Settings::default(),
            // The wall clock starts from the same default set-point at the boot origin; the host's
            // `set_settings` re-stamps it from the persisted clock a moment later.
            wall_clock: WallClock::new(Settings::default().local_clock()),
            settings_dirty: false,
            hold_progress_override: None,
            hold_cancel_pending: false,
            last_battery_poll_ms: None,
            temp_c: None,
            last_fix_ms: None,
            prev_no_fix: true,
            poi_scratch: screen::PoiScratch::new(),
            ble_passkey: None,
            store_changed_pending: 0,
            pending_upload: None,
            pending_warnings: WarningFlags::NONE,
            warned: WarningFlags::NONE,
        }
    }

    /// Build the idle power-on [`App`] **in place** at `slot` — the by-reference twin of
    /// [`new_idle`](App::new_idle), the placement path the firmware uses to construct the ~200 KB
    /// resident `App` straight into its reserved region without materializing it (or its renderer
    /// scratch) on the 192 KB stack.
    ///
    /// `new_idle` returns by value and only stays off the stack via return-value optimization — a
    /// fragile guarantee a debug build or different toolchain could drop, overflowing the stack.
    /// This writes each field through `addr_of_mut!` exactly once, so no by-value `App` is ever
    /// formed. The renderer (the only large field) is zeroed in place via
    /// [`MapRenderer::init_zeroed`] rather than built-and-moved.
    ///
    /// The end state is identical to `new_idle`'s — keep the two in sync.
    ///
    /// # Safety
    /// `slot` must be a valid, aligned `*mut App` the caller exclusively owns and into which a full
    /// `App` may be written. On return the slot is fully initialized; read it via `&mut *slot`.
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
            addr_of_mut!((*slot).catalog_ids).write(heapless::Vec::new());
            addr_of_mut!((*slot).ride_catalog).write(crate::ride::RideCatalog::new());
            addr_of_mut!((*slot).ride_catalog_ids).write(heapless::Vec::new());
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
            addr_of_mut!((*slot).region_dirty).write(None);
            addr_of_mut!((*slot).frame_size).write((0, 0));
            addr_of_mut!((*slot).render_clip).write(None);
            addr_of_mut!((*slot).next_wake_ms).write(None);
            addr_of_mut!((*slot).settings).write(Settings::default());
            addr_of_mut!((*slot).wall_clock).write(WallClock::new(Settings::default().local_clock()));
            addr_of_mut!((*slot).settings_dirty).write(false);
            addr_of_mut!((*slot).hold_progress_override).write(None);
            addr_of_mut!((*slot).hold_cancel_pending).write(false);
            addr_of_mut!((*slot).last_battery_poll_ms).write(None);
            addr_of_mut!((*slot).temp_c).write(None);
            addr_of_mut!((*slot).last_fix_ms).write(None);
            addr_of_mut!((*slot).prev_no_fix).write(true);
            addr_of_mut!((*slot).poi_scratch).write(screen::PoiScratch::new());
            addr_of_mut!((*slot).ble_passkey).write(None);
            addr_of_mut!((*slot).store_changed_pending).write(0);
            addr_of_mut!((*slot).pending_upload).write(None);
            addr_of_mut!((*slot).pending_warnings).write(WarningFlags::NONE);
            addr_of_mut!((*slot).warned).write(WarningFlags::NONE);
        }
    }

    /// Build the **map-first** [`App`] in place at `slot` — the by-reference twin of
    /// [`new`](App::new), as [`init_idle`](App::init_idle) is the twin of
    /// [`new_idle`](App::new_idle). Initialises the idle state, then drops straight onto the live
    /// Map (stack `[Home, Map]`, Riding) — the placement path a firmware bring-up uses to put the
    /// map on glass before buttons exist.
    ///
    /// # Safety
    /// Same contract as [`init_idle`](App::init_idle).
    pub unsafe fn init_map(slot: *mut App, state: AppState) {
        // SAFETY: caller's contract. `init_idle` fully initialises the slot, so thereafter
        // `&mut *slot` is sound and the map-first tail is plain safe mutation (assignment drops the
        // just-written Idle activity, not leaks it).
        unsafe { Self::init_idle(slot, state) };
        let app = unsafe { &mut *slot };
        app.activity = Activity::new(Mode::Riding);
        let _ = app.stack.push(Screen::Map(MapScreen::new()));
    }

    /// Advance one tick from the sensors.
    ///
    /// Polls the GPS [`LocationSource`] (recenters the camera in Follow mode) and, with a route
    /// loaded, snaps the fix onto it via [`RouteMatch`] and integrates ridden distance / moving
    /// time. Separately polls the barometer for climb — the streams are asynchronous, so each
    /// accumulates on its own cadence.
    ///
    /// `clock` is the [`RideClock`] (fix-consistent millis) so moving-time isn't scaled by the sim's
    /// replay multiplier; button holds use [`InputClock`] in [`handle_input`](App::handle_input).
    /// Loading or swapping a route resets the matcher and ride totals here, once per load.
    pub fn tick(&mut self, clock: RideClock, sensors: Sensors, route: Option<&RouteReader>) {
        let now_ms = clock.0;
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
        // Mirror the active route's length for the riding views (0 when none loaded). A change here
        // means the *drawable* route appeared or vanished — a load, or a transient SD glitch
        // recovering where the geometry becomes streamable a frame or two later. Dirty the map so
        // the route line is painted (or cleared) even on a frame with no fresh fix.
        let route_total_before = self.activity.route_total_m;
        self.activity.route_total_m = route.map_or(0, |r| r.total_distance_m);
        if self.activity.route_total_m != route_total_before {
            self.map_dirty = true;
        }

        let Sensors { loc, altimeter, temperature, clock, compass, track, fuel } = sensors;
        // Battery charge from the PMIC gauge, on the slow ~30 s cadence. A reading only repaints
        // Home — the one screen that draws the gauge — when the level **actually changes** (the
        // `shows_live_data` gate below is for the riding views, not Home, so dirty it here).
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
        // The state before this tick's *fix*, snapshotted **after** the battery poll so a pure
        // battery delta is never mistaken for a fix that moved the camera / marker / heading (which
        // one `AppState` comparison below detects). `battery_pct` lives in `AppState` but is drawn
        // only on Home, so it has the Home-only gate above; counting it toward `shows_live_data`
        // would force a full ~97 ms map render every 30 s on the riding views that don't draw it.
        let state_before = self.state;
        // Barometric altitude → climb + the elevation stamped on the log. Polled before the fix so a
        // point logged this tick carries the freshest altitude.
        if let Some(altimeter) = altimeter {
            if let Some(alt) = altimeter.poll() {
                self.activity.record_altitude(alt);
            }
        }
        // Ambient temperature: on device the BMP581 reports it free alongside the per-fix pressure
        // read. Stored off `AppState` (no screen draws it yet) so it never gates a map redraw.
        if let Some(temperature) = temperature {
            if let Some(c) = temperature.poll() {
                self.temp_c = Some(c);
            }
        }
        // GPS UTC time → the wall clock, but **only** in "Set from GPS" mode (so a manual clock is
        // never overwritten). The receiver resolves time before a 3D position, so this lands during
        // acquisition — the clock can be right while the "No GPS Fix" banner is still up. Not flagged
        // `settings_dirty`: don't persist on every fix (the set-point self-heals from GPS each boot;
        // a per-second RRAM write would thrash the store).
        if self.settings.gps_time {
            if let Some(t) = clock.and_then(|c| c.poll()) {
                self.settings.clock = t.utc;
                // Stamp against the **map-plane** clock `self.now_ms` (not the sensor-timebase
                // `RideClock`), the clock `WallClock::now` is later read with. Back-date by the
                // seconds-into-the-minute so the displayed minute rolls at the true instant, not up
                // to a fix-interval late.
                let epoch = self.now_ms.wrapping_sub(t.second as u32 * 1000);
                self.wall_clock.set(self.settings.local_clock(), epoch);
            }
        }
        // GPS fix → camera + map-match + ridden distance/time (only on a fresh fix, so a dropout
        // doesn't re-run the matcher or double-count). A *logged* fix also feeds the breadcrumb +
        // ride log.
        if let Some(fix) = self.state.update(loc) {
            // Stamp the fix-freshness clock against `self.now_ms` — the map-plane clock the banner's
            // staleness check + render read with. Off `AppState`, so a stationary fix that moves
            // nothing doesn't force a redraw here.
            self.last_fix_ms = Some(self.now_ms);
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
        // Electronic compass → the heading when the GPS can't give a course. Polled after the fix so
        // it sees this tick's movement state, and adopted *only* when it would actually drive the
        // orientation: heading-up, not panning, and the latest fix has no course (stopped). Storing
        // it in any other state (where `course_rad` ignores it) would change `state` on every
        // reading and force a needless map redraw.
        if let Some(compass) = compass {
            if let Some(heading) = compass.poll() {
                let stopped = self.state.user_fix.and_then(|f| f.course).is_none();
                if stopped && self.state.heading_up && self.state.pan.is_none() {
                    self.state.compass_deg = Some(heading);
                }
            }
        }
        // A fresh fix that moved the camera, marker or heading dirties the map — but only on a
        // screen that *draws* live data (Map / Statistics). On Home and the menus the camera still
        // follows the fix, but nothing they draw uses it, so a fix there must not redraw them. The
        // `AppState` comparison also makes a stationary fix a no-op. (The breadcrumb only grows on a
        // moving logged fix, which moved `user_fix` too, so it's covered by the same comparison.)
        if self.state != state_before && self.shows_live_data() {
            self.map_dirty = true;
        }
        // The "No GPS Fix" banner flips on a *timer* — a fix going stale (lost), or the
        // first/returning fix (acquired) — which the `state` comparison can miss (a fix lost to
        // silence is no state change at all). Surface that edge at the **end** of `tick` (after
        // `last_fix_ms` is stamped, every frame) so it reads the exact `no_fix` the render will,
        // dirtying only the live-data views, once per flip.
        let no_fix = !self.has_live_fix(self.now_ms);
        if no_fix != self.prev_no_fix {
            self.prev_no_fix = no_fix;
            if self.shows_live_data() {
                self.map_dirty = true;
            }
        }
    }

    /// Whether the base screen shows live sensor data (user fix / ride accumulators) — Map and
    /// Statistics do, so a fresh fix must redraw them; Home and the menus don't. The base is the
    /// lowest *opaque* drawn screen, so an overlay (Ride control) over a riding view still counts as
    /// live since the map keeps moving under the pause panel.
    fn shows_live_data(&self) -> bool {
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        matches!(self.stack.get(base), Some(Screen::Map(_) | Screen::Statistics(_)))
    }

    /// Whether the base (lowest opaque) screen draws the **map** — the [`Map`](crate::screen::map)
    /// screen, the only one that reads the streamed-map [`Reader`]. A render-on-demand host polls
    /// this to skip the whole map pipeline on a non-map frame: don't build the `Reader` (an SD
    /// style-table parse + its stack spike), pass `None` to
    /// [`render_map_timed`](App::render_map_timed), and a menu / Home redraw draws only its own
    /// chrome with zero map I/O.
    pub fn base_draws_map(&self) -> bool {
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        matches!(self.stack.get(base), Some(Screen::Map(_)))
    }

    /// Whether the frame needs the streamed-map [`Reader`] built and passed to
    /// [`render_map_timed`](App::render_map_timed) — a superset of [`base_draws_map`](App::base_draws_map).
    /// The Map always does; the **POI list** screen (issue #425) does too, but only until it has
    /// taken its one-shot snapshot; and the **POI detail** screen (issue #444) does until it has
    /// resolved its one hours read. Both read the `Reader` in the *draw* path off `rx.reader`, so a
    /// render-on-demand host (the board's two-plane loop) must build the `Reader` on the frame each
    /// one-shot read is taken. Once the list's [`poi_snapshot_pending`](App::poi_snapshot_pending) is
    /// false — or the detail's schedule cache has resolved — the screen draws from its frozen
    /// state with no `Reader`, so the host skips the build again.
    ///
    /// The sim's `render_frame` always passes `Some(reader)`, so it never consults this — only the
    /// board host does, keeping its per-frame `Reader` build (and stack spike) off every non-map,
    /// already-resolved frame.
    pub fn base_needs_reader(&self) -> bool {
        if self.base_draws_map() {
            return true;
        }
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        match self.stack.get(base) {
            Some(Screen::PoiList(s)) => self.poi_snapshot_pending(s),
            // The detail's hours read runs at draw off `rx.reader`; keep it built until it lands.
            Some(Screen::PoiDetail(s)) => s.hours_pending(),
            _ => false,
        }
    }

    /// Whether the given POI list screen still needs a `Reader` at draw — its category's snapshot
    /// hasn't been taken into the shared scratch yet. Drives [`base_needs_reader`](App::base_needs_reader).
    fn poi_snapshot_pending(&self, screen: &crate::screen::PoiListScreen) -> bool {
        !self.poi_scratch.holds(screen.category())
    }

    /// The fix-staleness window (map-plane millis): the larger of [`NO_FIX_FLOOR_MS`] and a few
    /// configured fix intervals, so a long interval doesn't flag "no fix" in the normal gap between
    /// its own fixes. A 1 s interval gives the 5 s floor; a 30 s interval gives 90 s.
    fn no_fix_window_ms(&self) -> u32 {
        (self.settings.fix_interval_s as u32 * 1000 * NO_FIX_INTERVALS).max(NO_FIX_FLOOR_MS)
    }

    /// Whether there's a **current** GPS fix at `now_ms`: a fix has been accepted and is no older
    /// than [`no_fix_window_ms`](App::no_fix_window_ms). `false` before the first fix (acquiring)
    /// and once the signal drops (lost) — exactly when the "No GPS Fix" banner shows.
    pub fn has_live_fix(&self, now_ms: u32) -> bool {
        self.last_fix_ms.is_some_and(|t| now_ms.wrapping_sub(t) <= self.no_fix_window_ms())
    }

    /// Replace the resident route catalog from a host store without durable ids, assigning
    /// **positional** ids (`0..n`). Everything indexed remaps by position — i.e. an index that is
    /// still in range survives, one past the end falls back — which is the sanest reading of an
    /// id-less store. Hosts with real object identity (the firmware's filename-encoded ids, the
    /// sim's session ids) call [`set_routes_with_ids`](App::set_routes_with_ids) instead; don't mix
    /// the two on one `App`, or a positional id will remap against a durable one.
    pub fn set_routes(&mut self, summaries: &[RouteSummary]) {
        let mut ids: heapless::Vec<u16, { crate::route::MAX_ROUTES }> = heapless::Vec::new();
        for i in 0..summaries.len().min(crate::route::MAX_ROUTES) {
            let _ = ids.push(i as u16);
        }
        self.set_routes_with_ids(summaries, &ids);
    }

    /// Replace the resident route catalog from the host's store, carrying each route's **durable
    /// object id** (`ids` parallel to `summaries`), then remap every held catalog index by id
    /// (#450). Clones up to [`MAX_ROUTES`](crate::MAX_ROUTES) entries; any beyond that are ignored.
    ///
    /// The remap is the live-catalog contract: a rescan that inserts or removes a route re-points
    /// [`Activity::active_route`], the matcher/profile caches keyed on it, an open Route-menu
    /// selection, a Route-overview preview, and a pending
    /// [`RouteSwapScreen`](crate::screen::RouteSwapScreen) at the *same route* (by id) in the new
    /// order. A vanished route falls back sanely: navigation unloads (`active_route = None`, stale
    /// matcher progress + profile dropped), a menu selection clamps near its old position, a
    /// preview/swap subject turns into its screen's own missing-route path. Dirties the map once —
    /// a store change is a repaint-worthy host event (the open menu refreshes in place).
    pub fn set_routes_with_ids(&mut self, summaries: &[RouteSummary], ids: &[u16]) {
        let old_ids = self.catalog_ids.clone();
        self.catalog.clear();
        self.catalog_ids.clear();
        for (s, &id) in summaries.iter().zip(ids).take(crate::route::MAX_ROUTES) {
            let _ = self.catalog.push(s.clone());
            let _ = self.catalog_ids.push(id);
        }
        self.remap_route_indices(&old_ids);
        self.map_dirty = true;
    }

    /// Re-point every held catalog index after the catalog was replaced: old index → its id in
    /// `old_ids` → that id's new index (or `None` if the route vanished). See
    /// [`set_routes_with_ids`](App::set_routes_with_ids).
    fn remap_route_indices(&mut self, old_ids: &[u16]) {
        let new_ids = &self.catalog_ids;
        let remap = |i: usize| -> Option<usize> {
            let id = *old_ids.get(i)?;
            new_ids.iter().position(|&x| x == id)
        };

        // The navigated route + the caches keyed on it. When the identity survives, all three move
        // together, so nothing resets (no matcher re-lock, no profile rebuild). When it vanished,
        // navigation unloads and the stale per-route state is dropped with it.
        let old_active = self.activity.active_route;
        self.activity.active_route = old_active.and_then(remap);
        if old_active.is_some() && self.activity.active_route.is_none() {
            self.route_match.reset(); // drop stale progress/off-route from the vanished route
        }
        self.matched_route = self.matched_route.and_then(remap);
        let old_profile = self.profile_route;
        self.profile_route = old_profile.and_then(remap);
        if old_profile.is_some() && self.profile_route.is_none() {
            self.profile = None;
        }

        // Every screen on the stack that holds a catalog index.
        let new_len = new_ids.len();
        for s in self.stack.iter_mut() {
            match s {
                Screen::RouteMenu(m) => m.remap_routes(&remap, new_len),
                Screen::RouteOverview(o) => o.remap_routes(&remap),
                Screen::RouteSwap(sw) => sw.remap_routes(&remap),
                Screen::RouteReceived(rc) => rc.remap_routes(&remap),
                Screen::RouteUpdated(ru) => ru.remap_routes(&remap),
                _ => {}
            }
        }
    }

    /// The resident route catalog.
    pub fn routes(&self) -> &[RouteSummary] {
        &self.catalog
    }

    /// Each catalog entry's durable object id, parallel to [`routes`](App::routes) — as last fed to
    /// [`set_routes_with_ids`](App::set_routes_with_ids) (positional for plain
    /// [`set_routes`](App::set_routes)).
    pub fn route_ids(&self) -> &[u16] {
        &self.catalog_ids
    }

    /// Drain the Route menu's pending route-delete request (epic #447, P6), resolved to the route's
    /// **durable object id**. The host calls this once per pass; a `Some(id)` is its cue to delete
    /// that route object (`ObjectStore::delete_route` on the board, the routes-dir file on the sim) —
    /// the resulting store-changed edge re-feeds the catalog with the route gone, and P3's identity
    /// remap keeps `active_route` / the menu selection pointing at the right routes.
    ///
    /// The request is recorded as a catalog **index** (what the screen holds) and translated here
    /// against the live [`route_ids`](App::route_ids), so a rescan racing between the hold and this
    /// drain can never resolve to the wrong route: a still-present index yields its id, an
    /// out-of-range one (the route already vanished) drains to `None`.
    pub fn take_route_delete(&mut self) -> Option<u16> {
        let idx = self.activity.take_route_delete()?;
        self.catalog_ids.get(idx).copied()
    }

    /// Non-consuming peek at whether a route-delete request is pending — lets the board gate its
    /// per-pass store work on actual change without draining the one-shot (mirrors
    /// [`Activity::has_track_action`](crate::Activity::has_track_action)).
    pub fn has_route_delete(&self) -> bool {
        self.activity.has_route_delete()
    }

    /// Replace the resident **ride** catalog from the host's store (epic #447, P7), carrying each
    /// ride's durable object id (`ids` parallel to `summaries`) and its `synced` flag (baked into the
    /// summary by the host from the SD synced-set sidecar). Re-points an open Rides-menu selection by
    /// id across the rescan, so a finished ride, a phone-side ride delete, or an on-device delete
    /// appears/disappears without a reboot. Clones up to [`MAX_RIDES`](crate::MAX_RIDES); any beyond
    /// that are ignored. Sorted-by-`start_time` is the host's job (the board scan and the sim store
    /// both hand newest-first). Dirties the map once — a store change is a repaint-worthy event.
    pub fn set_rides(&mut self, summaries: &[RideSummary], ids: &[u16]) {
        let old_ids = self.ride_catalog_ids.clone();
        self.ride_catalog.clear();
        self.ride_catalog_ids.clear();
        for (s, &id) in summaries.iter().zip(ids).take(crate::ride::UI_RIDES_CAP) {
            let _ = self.ride_catalog.push(s.clone());
            let _ = self.ride_catalog_ids.push(id);
        }
        // Re-point the Rides menu's highlight by identity (its id in `old_ids` → new index), the
        // ride-namespace twin of the route remap. Only the Rides screen holds a ride index.
        let new_ids = &self.ride_catalog_ids;
        let remap = |i: usize| -> Option<usize> {
            let id = *old_ids.get(i)?;
            new_ids.iter().position(|&x| x == id)
        };
        let new_len = new_ids.len();
        for s in self.stack.iter_mut() {
            if let Screen::Rides(m) = s {
                m.remap_rides(&remap, new_len);
            }
        }
        self.map_dirty = true;
    }

    /// The resident ride catalog (summaries) — what the Rides screen lists.
    pub fn rides(&self) -> &[RideSummary] {
        &self.ride_catalog
    }

    /// Each ride-catalog entry's durable object id, parallel to [`rides`](App::rides) — as last fed to
    /// [`set_rides`](App::set_rides).
    pub fn ride_ids(&self) -> &[u16] {
        &self.ride_catalog_ids
    }

    /// Drain the Rides screen's pending ride-delete request (epic #447, P7), resolved to the ride's
    /// **durable object id** — the ride-namespace twin of [`take_route_delete`](App::take_route_delete).
    /// A `Some(id)` is the host's cue to delete that ride object (`ObjectStore::delete_ride` on the
    /// board, the tracks-dir file on the sim); the store-changed edge re-feeds the ride catalog with
    /// it gone. Resolved against the live [`ride_ids`](App::ride_ids), so a rescan racing the hold
    /// drains a vanished ride to `None` rather than the wrong id.
    pub fn take_ride_delete(&mut self) -> Option<u16> {
        let idx = self.activity.take_ride_delete()?;
        self.ride_catalog_ids.get(idx).copied()
    }

    /// Non-consuming peek at whether a ride-delete request is pending — lets the board gate its
    /// per-pass store work without draining the one-shot.
    pub fn has_ride_delete(&self) -> bool {
        self.activity.has_ride_delete()
    }

    /// Drain the pending **route-planning request** (epic #116, R4) — the POI create-route
    /// confirm's one-shot. A `Some` is the host's cue to run
    /// [`plan_route`](obc_route::nav::plan_route) from `from` to `to` against its map, write the
    /// emitted OBCR to the reserved nav route (`/routes/_nav.obcr`), rescan the catalog
    /// ([`set_routes_with_ids`](App::set_routes_with_ids)), and answer with
    /// [`notify_nav_result`](App::notify_nav_result) — all within the same pass, so the confirm
    /// screen is still up when the answer lands.
    pub fn take_nav_request(&mut self) -> Option<crate::activity::NavRequest> {
        self.activity.take_nav_request()
    }

    /// Non-consuming peek at whether a route-planning request is pending — lets the board gate
    /// its per-pass router work without draining the one-shot.
    pub fn has_nav_request(&self) -> bool {
        self.activity.has_nav_request()
    }

    /// **Debug bench** (#500): start a route plan from `from` to `to` (both `(lon, lat)` µdeg) exactly
    /// as the POI create-route confirm does — record the [`NavRequest`](crate::activity::NavRequest)
    /// **and** push the planning screen — so the host steps the resumable router with the same live
    /// spinner + between-step render cadence the rider sees, and the `nav route:` RTT line reflects the
    /// real user-perceived cost. Only wired on the `debug-uart` build (driven by the `N` VCOM command);
    /// no UI path reaches it.
    pub fn debug_start_nav(&mut self, from: (i32, i32), to: (i32, i32), name: &str) {
        self.activity.request_nav(crate::activity::NavRequest::new(from, to, name));
        // At most one planning screen, ever: the bench host repeats the `N` line (the VCOM RX is
        // flaky) and each repeat lands as a fresh request — but the answer replaces only the
        // *first* planning screen it finds, so a second push here would survive it and spin
        // forever (measured: a permanent ~9 Hz full-chrome repaint after the plan).
        if !self.stack.iter().any(|s| matches!(s, Screen::NavPlanning(_))) {
            let _ = self.stack.push(Screen::NavPlanning(crate::screen::NavPlanningScreen::new(name)));
        }
        self.map_dirty = true;
    }

    /// Drain the pending **plan-cancel request** (#499) — recorded by the planning screen's Back
    /// (which already popped back to the POI detail). A `true` is the host's cue to abort the
    /// in-flight plan and discard the partial nav file; it answers **nothing** (there is no
    /// planning screen left to answer into). Drained once per pass; a stale cancel with no plan
    /// in flight is a no-op.
    pub fn take_nav_cancel(&mut self) -> bool {
        self.activity.take_nav_cancel()
    }

    /// The host's answer to a drained [`take_nav_request`](App::take_nav_request) (epic #116, R4).
    ///
    /// `Ok(id)` is the committed nav route's **durable object id**, resolved against the already
    /// rescanned catalog (the same ordering contract as
    /// [`notify_route_uploaded`](App::notify_route_uploaded): rescan first, then this). The route
    /// **activates** — [`Activity::active_route`] points at it so the host streams its geometry —
    /// and the confirm screen is replaced by the computed-route
    /// [overview](crate::screen::RouteOverviewScreen) (length only). Because the reserved nav file
    /// is overwritten in place, a re-route can commit **new bytes under the same id**; every cache
    /// derived from the old geometry (matcher lock, elevation profile, match readouts) is dropped
    /// unconditionally, the forced-adoption discipline of an active replace.
    ///
    /// `Err` swaps the confirm for the failure card — the locked two tiers:
    /// [`Exhausted`](obc_route::nav::NavError::Exhausted) → "Too far to route here" (there is no
    /// distance cap — running out of the router's fixed table **is** the device's range limit),
    /// everything else → "Couldn't find a route."
    ///
    /// The answer lands in the **planning screen** (#499 — the confirm swapped to it when the
    /// request was recorded). If it's gone — the rider cancelled (Back popped it), or a
    /// host-pushed card replaced it — the answer is dropped: a committed route is still in the
    /// Route menu, and a cancel already told the host to abort before any answer.
    pub fn notify_nav_result(&mut self, result: Result<u16, obc_route::nav::NavError>) {
        use obc_route::nav::NavError;
        let Some(i) = self.stack.iter().position(|s| matches!(s, Screen::NavPlanning(_))) else {
            return;
        };
        // Resolve the id in the (already rescanned) catalog; a missing id degrades to the
        // generic failure tier.
        let resolved = result.and_then(|id| self.catalog_ids.iter().position(|&x| x == id).ok_or(NavError::NoPath));
        let screen = match resolved {
            Ok(idx) => {
                // New bytes may sit under a same-id reserved file (a re-route): drop everything
                // derived from the old geometry so the matcher re-locks and the profile rebuilds
                // from the fresh route — cheap, runs once per plan.
                self.route_match.reset();
                self.matched_route = None;
                self.profile = None;
                self.profile_route = None;
                self.activity.progress_m = 0;
                self.activity.off_route = false;
                self.activity.dist_to_route_m = 0;
                // Activate for the preview (the overview contract: the host streams the geometry
                // while the page shows); `prev_active` restores whatever was loaded on cancel.
                let prev = self.activity.active_route;
                self.activity.active_route = Some(idx);
                Screen::RouteOverview(crate::screen::RouteOverviewScreen::computed(idx, prev))
            }
            // Exhaustion is the device's honest "too far" — the range tier's trigger now that
            // there is no crow-flies cap; everything else is the generic tier.
            Err(NavError::Exhausted) => Screen::NavFail(crate::screen::NavFailScreen::too_far()),
            Err(_) => Screen::NavFail(crate::screen::NavFailScreen::not_found()),
        };
        self.stack[i] = screen;
        self.map_dirty = true;
    }

    /// Feed the host's BLE link snapshot ([`BleStatus`](crate::BleStatus)) — the host→app event seam
    /// (epic #447). The board's BLE plane distils its `ble::state` into this each pass; the simulator
    /// injects it from the control panel. Called like [`set_routes`](App::set_routes): a plain host
    /// event, no BLE crate type crossing the boundary.
    ///
    /// A change in the link phase or the paired flag dirties the map so the drawn state repaints —
    /// but only where it's actually drawn (the menu title bar / Home / the Bluetooth screen), via
    /// the same `AppState`-comparison gate the riding views use, so an unchanged status (the steady
    /// state, fed every pass) repaints nothing.
    ///
    /// The **passkey** (epic #447, P2) drives the host-pushed [`PasskeyScreen`](crate::screen::PasskeyScreen):
    /// a passkey going `Some` opens the card over whatever is up, and its clearing (pairing
    /// complete/failed, or disconnect — all cleared BLE-side) closes it. Fed every pass with an
    /// unchanged status, [`reconcile_passkey_card`](App::reconcile_passkey_card) is a no-op, so the
    /// steady state never re-dirties. Because it's a host-pushed screen, it also **defers while a
    /// hold is charging** (yanking the hold target out from under the rider mid-charge would break
    /// the confirm) — the reconcile just skips that pass and lands on the next, since the desired
    /// state is re-fed every pass.
    pub fn set_ble_status(&mut self, status: crate::ble::BleStatus) {
        let state_before = self.state;
        self.state.ble_link = status.link;
        self.state.ble_paired = status.paired;
        // The link state lives in `AppState` but is drawn only on Home, the menu title bars, and
        // the Bluetooth screen, so — like the Home-only battery gate — a change dirties the map
        // only when one of those is the base screen. Counting it toward `shows_live_data` would
        // force a full map render on the riding views, which never draw the indicator.
        if self.state != state_before && self.indicator_visible() {
            self.map_dirty = true;
        }
        self.ble_passkey = status.passkey;
        self.reconcile_passkey_card();
    }

    /// Open or close the host-pushed passkey card to match the seam's passkey ([`ble_passkey`](App::ble_passkey)):
    /// push a [`PasskeyScreen`](crate::screen::PasskeyScreen) when a passkey is present and no card is
    /// up, remove it when the passkey clears. Idempotent — the steady state (same passkey re-fed each
    /// pass) does nothing, so it never re-dirties. **Deferred while a hold charges** so a host-pushed
    /// screen never lands mid-hold (push *or* pop); the desired state is re-fed every pass, so the
    /// deferral is simply "try again next pass". Each transition dirties the map exactly once: opening
    /// covers the screen below (its own draw); closing repaints whatever the card covered.
    ///
    /// The card outranks the P4 route-upload popups: a popup consults
    /// [`passkey_card_up`](App::passkey_card_up) and drops its prompt while the card is showing.
    fn reconcile_passkey_card(&mut self) {
        // Never move a host-pushed screen onto/off the stack while a hold is charging.
        if self.hold_charging() {
            return;
        }
        match (self.ble_passkey, self.passkey_card_index()) {
            // A passkey to show and no card up → open it over the current top. The card outranks
            // the route-upload popups (P4) in *both* directions: a popup arriving under the card
            // is dropped (see `reconcile_upload_prompt`), and a passkey arriving while a popup is
            // up **replaces** it — remove the popup rather than stacking the card over it (it's
            // advisory; the route is in the menu either way). The manual, menu-opened Route-swap
            // prompt is not a popup and stays put under the card.
            (Some(passkey), None) => {
                self.remove_received_popups();
                let r = self.stack.push(Screen::Passkey(crate::screen::PasskeyScreen::new(passkey)));
                debug_assert!(r.is_ok(), "screen stack overflow — raise MAX_DEPTH");
                self.map_dirty = true;
            }
            // No passkey but a card is up → remove it wherever it sits (the rider may not have
            // touched anything), and repaint what it covered.
            (None, Some(i)) => {
                let _ = self.stack.remove(i);
                self.map_dirty = true;
            }
            // Card already matches the passkey (both present, or both absent): nothing to do.
            _ => {}
        }
    }

    /// The stack index of the passkey card, or `None` when it isn't up. The card only ever sits as
    /// the top (it swallows input, and nothing navigates past it), but this searches the whole stack
    /// so a close removes it wherever it ended up.
    fn passkey_card_index(&self) -> Option<usize> {
        self.stack.iter().position(|s| matches!(s, Screen::Passkey(_)))
    }

    /// Whether the passkey card is currently up (epic #447). The P4 route-upload popups poll this to
    /// honour the priority rule — a popup is dropped, not queued, while the card shows.
    pub fn passkey_card_up(&self) -> bool {
        self.passkey_card_index().is_some()
    }

    /// Whether a hold gesture is charging right now — either button down, its long-press not yet
    /// fired. Reads the host-fed encoder progress ([`set_hold_progress`](App::set_hold_progress), the
    /// two-plane firmware) and `App`'s own input plane (the single-loop hosts). Gates the host-pushed
    /// passkey card's open/close so it never lands mid-hold.
    fn hold_charging(&self) -> bool {
        self.hold_progress_override.is_some_and(|p| p > 0.0)
            || self.input.encoder_hold_progress() > 0.0
            || self.input.back_hold_progress() > 0.0
    }

    /// Drain the Bluetooth screen's pending **"Forget phone"** request (epic #447, P8): `true` at
    /// most once per guarded hold. The board's ride loop rings the BLE plane (clear the RRAM bond
    /// slot + drop the bonded connection); the sim clears its injected `paired` flag. The
    /// `TrackAction` shape: a one-shot the host consumes, not a level.
    pub fn take_ble_forget(&mut self) -> bool {
        core::mem::take(&mut self.state.ble_forget_pending)
    }

    /// Whether the base (lowest opaque) screen draws the connected indicator — Home, or any framed
    /// screen with a title bar (a menu / list / prompt), i.e. everything that isn't a full-screen
    /// riding view. Gates [`set_ble_status`](App::set_ble_status)'s repaint so a link change never
    /// re-renders the map on the Map / Statistics screens, which deliberately omit the glyph.
    fn indicator_visible(&self) -> bool {
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        !matches!(self.stack.get(base), Some(Screen::Map(_) | Screen::Statistics(_)))
    }

    /// The live BLE pairing passkey, or `None` when not pairing — [`BleStatus::passkey`](crate::BleStatus)
    /// as last fed to [`set_ble_status`](App::set_ble_status). Consumed by the passkey card in P2
    /// (#449); exposed now so the seam is observable end to end.
    pub fn ble_passkey(&self) -> Option<u32> {
        self.ble_passkey
    }

    /// Signal that the object store committed or deleted an object (epic #447) — rung from the board's
    /// `ObjectStore` commit/delete paths, the same edge that notifies the phone's `storeChanged`.
    ///
    /// Records the pending signal; the host drains it via
    /// [`take_store_changed`](App::take_store_changed) and answers with a `/routes` rescan →
    /// [`set_routes_with_ids`](App::set_routes_with_ids) (the live catalog + identity remap, #450).
    pub fn notify_store_changed(&mut self) {
        self.store_changed_pending = self.store_changed_pending.saturating_add(1);
    }

    /// How many [`notify_store_changed`](App::notify_store_changed) signals are pending (not yet acted
    /// on). Non-zero once the store has moved since the last drain. A read-only observation hook for
    /// the board wiring and the seam tests; the acting consumer is
    /// [`take_store_changed`](App::take_store_changed).
    pub fn store_changed_pending(&self) -> u32 {
        self.store_changed_pending
    }

    /// Drain the pending store-changed signals (#450): returns the count and resets it. The host
    /// calls this once per pass and, when non-zero, rescans its store and re-feeds
    /// [`set_routes_with_ids`](App::set_routes_with_ids). A count (not a bool) so a burst of
    /// commits is observable, though one rescan covers them all.
    pub fn take_store_changed(&mut self) -> u32 {
        core::mem::take(&mut self.store_changed_pending)
    }

    /// A route upload **committed** to the host's store (epic #447, P4) — the event that raises
    /// the route-upload popups. `id` is the committed route's durable object id; `replaced` says
    /// the upload swapped the bytes of an already-stored route rather than adding a new one.
    ///
    /// **Ordering contract**: the host rings this *after* it has answered the accompanying
    /// store-changed edge with the rescan → [`set_routes_with_ids`](App::set_routes_with_ids), so
    /// `id` resolves against the fresh catalog (the board's ride loop drains the rescan first in
    /// the same pass; the sim's inject button drives the same sequence).
    ///
    /// Two things happen here, deliberately decoupled:
    ///
    /// 1. **Forced adoption** (unconditional): if the replaced id is the actively-navigated route,
    ///    the bytes under navigation just changed — the same-id remap kept the matcher/profile
    ///    caches alive across the rescan, so they now describe *stale geometry*. Drop them: the
    ///    matcher re-locks and re-runs map-matching from the current fix on the next tick, the
    ///    profile rebuilds from the reopened geometry at the next render, and the match-derived
    ///    readouts (progress / off-route / cross-track) clear until recomputed. The recording
    ///    session is untouched. This runs even when the popup is suppressed.
    /// 2. **The advisory prompt**: queued (single slot — consecutive uploads replace it, most
    ///    recent wins) and delivered by [`reconcile_upload_prompt`](App::reconcile_upload_prompt):
    ///    the info-only "ROUTE UPDATED" card for an active replace, the retitled
    ///    [`RouteSwapScreen`](crate::screen::RouteSwapScreen) while tracking, or the idle
    ///    "ROUTE RECEIVED" prompt. Dropped while the passkey card shows; deferred a tick while a
    ///    hold charges.
    pub fn notify_route_uploaded(&mut self, id: u16, replaced: bool) {
        let active_id = self.activity.active_route.and_then(|i| self.catalog_ids.get(i).copied());
        let active_replace = replaced && active_id == Some(id);
        if active_replace {
            // Same index, same id — but new bytes. Invalidate everything derived from the old
            // geometry (the remap deliberately preserves same-id state; a replace is the one case
            // where that preservation would carry stale state onto new geometry).
            self.route_match.reset();
            self.matched_route = None; // tick re-locks the matcher from the current fix
            self.profile = None;
            self.profile_route = None; // the next render rebuilds from the reopened geometry
            self.activity.progress_m = 0;
            self.activity.off_route = false;
            self.activity.dist_to_route_m = 0;
            self.map_dirty = true; // the drawn route line + progress changed under the rider
        }
        self.pending_upload = Some(UploadEvent { id, active_replace });
        self.reconcile_upload_prompt();
    }

    /// Deliver (or drop) the pending route-upload prompt (epic #447, P4). Called on arrival and
    /// once per [`advance_animations`](App::advance_animations) pass, so a hold-deferred prompt
    /// lands on the next tick — the P2 host-pushed-screen precedent, adapted to a one-shot event
    /// (the pending slot *is* the re-fed desired state).
    ///
    /// The locked popup rules, in order:
    /// - **Passkey outranks**: while the card is up the prompt is dropped, not queued (advisory —
    ///   the route is in the Route menu regardless).
    /// - **Never lands mid-hold**: delivery waits a tick while either button's hold charges.
    /// - **Vanished id**: a route deleted between commit and delivery drops the prompt.
    /// - **Replace, don't stack**: an existing upload popup — or a manual
    ///   [`RouteSwapScreen`](crate::screen::RouteSwapScreen) opened from the menu — is replaced in
    ///   place by the new prompt (most recent wins; selection resets with the fresh screen).
    fn reconcile_upload_prompt(&mut self) {
        let Some(ev) = self.pending_upload else { return };
        if self.passkey_card_up() {
            self.pending_upload = None; // dropped, not queued — the card outranks
            return;
        }
        if self.hold_charging() {
            return; // defer a tick; retried from `advance_animations`
        }
        self.pending_upload = None;
        // Resolve the durable id in the (already rescanned) catalog; a vanished route drops the
        // advisory prompt entirely.
        let Some(idx) = self.catalog_ids.iter().position(|&x| x == ev.id) else { return };
        let screen = if ev.active_replace {
            Screen::RouteUpdated(crate::screen::RouteUpdatedScreen::new(idx, self.now_ms))
        } else if self.activity.is_tracking() {
            Screen::RouteSwap(crate::screen::RouteSwapScreen::received(idx, self.now_ms))
        } else {
            Screen::RouteReceived(crate::screen::RouteReceivedScreen::new(idx, self.now_ms))
        };
        match self.upload_prompt_index() {
            Some(i) => self.stack[i] = screen,
            None => {
                let r = self.stack.push(screen);
                debug_assert!(r.is_ok(), "screen stack overflow — raise MAX_DEPTH");
            }
        }
        self.map_dirty = true;
    }

    /// Raise one or more device [warnings](crate::screen::WarningScreen) (issue #504) — a missing
    /// sensor the host's I²C probe never answered, or a map that loaded but reads slowly because
    /// it's fragmented. Host-pushed, coalesced onto a single dismissable card; each distinct flag
    /// is surfaced **once per boot** (a dismissed card doesn't nag, but a genuinely new flag
    /// re-opens it). Call it whenever a fault is discovered — order and timing don't matter, the
    /// flags accumulate. A no-op for [`WarningFlags::NONE`].
    pub fn notify_warning(&mut self, flags: WarningFlags) {
        if flags.is_empty() {
            return;
        }
        self.pending_warnings |= flags;
        self.reconcile_warning();
    }

    /// Deliver (or defer) the pending [warnings](App::notify_warning). Called on arrival and once
    /// per [`advance_animations`](App::advance_animations) pass, so a warning deferred behind a
    /// passkey card or a live hold lands on a later tick — the [`reconcile_upload_prompt`] pattern.
    /// Only the not-yet-shown subset is surfaced (`pending & !warned`); it ORs into an open card or
    /// pushes a fresh one.
    fn reconcile_warning(&mut self) {
        let fresh = self.pending_warnings & !self.warned;
        if fresh.is_empty() {
            self.pending_warnings = WarningFlags::NONE; // nothing new — drop any stale re-raise
            return;
        }
        // Advisory: never cover the passkey card (it outranks) and never land mid-hold. Keep the
        // flags pending and retry from `advance_animations` once the card clears / the hold resolves.
        if self.passkey_card_up() || self.hold_charging() {
            return;
        }
        self.warned |= fresh;
        self.pending_warnings = WarningFlags::NONE;
        match self.warning_index() {
            Some(i) => {
                if let Screen::Warning(s) = &mut self.stack[i] {
                    s.add(fresh);
                }
            }
            None => {
                let r = self.stack.push(Screen::Warning(crate::screen::WarningScreen::new(fresh)));
                debug_assert!(r.is_ok(), "screen stack overflow — raise MAX_DEPTH");
            }
        }
        self.map_dirty = true;
    }

    /// The stack index of a live [warning card](crate::screen::WarningScreen), so a newly-discovered
    /// fault ORs into it rather than stacking a second card. `None` when no card is open.
    fn warning_index(&self) -> Option<usize> {
        self.stack.iter().position(|s| matches!(s, Screen::Warning(_)))
    }

    /// The stack index of the screen an incoming upload prompt **replaces**: any upload popup, or
    /// the manual Route-swap prompt (the locked "same rule when the manual swap is up"). `None`
    /// when the prompt should push fresh.
    fn upload_prompt_index(&self) -> Option<usize> {
        self.stack
            .iter()
            .position(|s| matches!(s, Screen::RouteReceived(_) | Screen::RouteUpdated(_) | Screen::RouteSwap(_)))
    }

    /// Remove every host-pushed upload popup from the stack (the passkey card just opened over
    /// them — card outranks). The **manual** Route-swap prompt is rider-opened, not a popup, and
    /// stays. Returns whether anything was removed.
    fn remove_received_popups(&mut self) -> bool {
        let mut removed = false;
        let mut i = 0;
        while i < self.stack.len() {
            let popup = match &self.stack[i] {
                Screen::RouteReceived(_) | Screen::RouteUpdated(_) => true,
                Screen::RouteSwap(s) => s.is_received(),
                _ => false,
            };
            if popup {
                let _ = self.stack.remove(i);
                removed = true;
            } else {
                i += 1;
            }
        }
        removed
    }

    /// Auto-close any upload popup past its 30 s deadline — **timeout = dismiss** (epic #447,
    /// P4): the popup is removed exactly as Back would, nothing else changes. Deferred while a
    /// hold charges (the P2 rule: never move a host-pushed screen mid-hold); the popups'
    /// `tick_timers` keep a short residual wake armed until the sweep lands.
    fn close_expired_upload_popups(&mut self) {
        if self.hold_charging() {
            return;
        }
        let now = self.now_ms;
        let mut i = 0;
        while i < self.stack.len() {
            let expired = match &self.stack[i] {
                Screen::RouteReceived(s) => s.expired(now),
                Screen::RouteUpdated(s) => s.expired(now),
                Screen::RouteSwap(s) => s.expired(now),
                _ => false,
            };
            if expired {
                let _ = self.stack.remove(i);
                self.map_dirty = true; // repaint what the popup covered
            } else {
                i += 1;
            }
        }
    }

    /// The screen currently on top of the stack (receiving input). Always present — the Home root is
    /// never popped. A read-only handle for a host/test that needs to know which screen is up.
    pub fn top_screen(&self) -> &Screen {
        self.stack.last().expect("the stack always has the Home root")
    }

    /// Number of POIs in the current [`poi_scratch`](App::poi_scratch) snapshot (0 when none has
    /// been taken). A test/introspection hook for the POIs browser's static snapshot.
    pub fn poi_snapshot_len(&self) -> usize {
        self.poi_scratch.len()
    }

    /// Re-roll the Home screensaver's contour pattern to `seed`. The app does this itself when the
    /// stack returns to Home; this is the host-facing hook for previewing a specific pattern.
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
        // Stamp the wall clock to the persisted *local* set-point as of now (boot millis), so it
        // resumes from the stored time. `local_clock` folds in the UTC offset in GPS mode, so the
        // Home clock shows local time, not the raw UTC anchor.
        self.wall_clock.set(self.settings.local_clock(), self.now_ms);
        self.settings_dirty = false;
    }

    /// The live device settings — read by the host to persist them, and by anything that needs
    /// the current units / clock / GPS-interval outside the screen draw path.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The last ambient temperature (°C), or `None` before the first reading / no thermometer. No
    /// screen draws it yet; exposed for a future readout and host introspection.
    pub fn temperature_c(&self) -> Option<f32> {
        self.temp_c
    }

    /// The live wall-clock time right now (see [`WallClock`]). What a screen draws as `HH:MM`;
    /// exposed for a host wanting the current time outside the draw path.
    pub fn wall_clock_now(&self) -> DateTime {
        self.wall_clock.now(self.now_ms)
    }

    /// The current **UTC** unix seconds, from the wall clock. The clock's set-point is local
    /// time, so in GPS mode the persisted UTC offset is folded back out; a hand-set clock knows
    /// no zone, so its local reading is served as-is.
    pub fn wall_unix_now(&self) -> u32 {
        let local = self.wall_clock.unix_now(self.now_ms);
        if self.settings.gps_time {
            (local as i64 - self.settings.utc_offset_min as i64 * 60) as u32
        } else {
            local
        }
    }

    /// The ride totals + wall-clock anchor for the Finish-time ride-object save, read in the same
    /// frame the host drains [`TrackAction::Save`](crate::TrackAction) so the anchor pairs with the
    /// log's last points.
    pub fn ride_stats(&self) -> obc_route::RideStats {
        obc_route::RideStats {
            distance_m: self.activity.ridden_m as u32, // float→int casts saturate
            moving_time_s: self.activity.moving_s as u32,
            avg_speed_cms: self.activity.avg_speed_cms(),
            climb_m: self.activity.climb_m() as u16,
            unix_at_anchor: self.wall_unix_now(),
            anchor_ms: self.now_ms,
        }
    }

    /// Whether a settings edit is pending persistence **and the user has left the settings
    /// subtree** — the host's cue to persist [`settings`](App::settings), checked **once per frame**
    /// after [`handle_input`](App::handle_input). Drains the flag when it fires.
    ///
    /// The save is **debounced to leaving the settings screens**: a stepper sweep would otherwise
    /// drive one store write *per detent* — on the device, dozens of blocking in-place RRAM line
    /// writes to the same address. This relies on the invariant that **only the settings screens
    /// mutate [`settings`](App::settings)**; the trade-off is that an edit left un-exited when power
    /// is cut is lost (which the single-slot store already tolerates).
    pub fn take_settings_dirty(&mut self) -> bool {
        if self.settings_dirty && !self.top_is_settings() {
            self.settings_dirty = false;
            true
        } else {
            false
        }
    }

    /// Whether the top (input-receiving) screen is one of the settings screens — the gate
    /// [`take_settings_dirty`](App::take_settings_dirty) uses to hold a pending save until exit.
    /// Reads the [`ScreenKind`](crate::screen::ScreenKind) each screen declares in its `screens!`
    /// table row, so a new settings screen can't be forgotten here.
    fn top_is_settings(&self) -> bool {
        self.stack.last().is_some_and(|s| s.kind().is_settings())
    }

    /// Whether the top screen would draw a live **hold fill** for its current selection/state —
    /// a guarded confirm row (Ride control, Route swap), the armed factory-Reset bar, or the
    /// Fields hold-to-delete footer over a deletable row. A render-on-demand host combines this
    /// with the charging hold-progress to redraw only when the fill would actually animate;
    /// holding the encoder on any other screen changes no pixels, so no repaint is owed.
    pub fn top_wants_hold_fill(&self) -> bool {
        self.stack.last().is_some_and(|s| {
            s.wants_hold_fill(
                &self.settings,
                &self.state,
                &self.activity,
                self.catalog.as_slice(),
                self.ride_catalog.as_slice(),
            )
        })
    }

    /// **Debug/benchmark hook** (the USB-CDC `Z` command): set the map camera to exactly `mpp`
    /// meters-per-pixel and force one map redraw. Drives the zoom directly (bypassing the encoder's
    /// fixed detents) so a render sweep can pin an exact scale per sample. Part of the strippable
    /// render-instrumentation seam.
    pub fn set_map_mpp(&mut self, mpp: f32) {
        self.state.zoom = zoom_for_mpp(mpp);
        self.map_dirty = true;
    }

    /// Recognise this frame's raw control input and apply each resulting gesture to the top screen,
    /// then advance the visible screens' timed content. Fuses the two planes into one call for the
    /// simulator and the single-executor firmware; `clock` is the [`InputClock`] for hold timing.
    /// Call once per frame even with no pending events — that is how a held button's long-press
    /// fires.
    ///
    /// The two-plane firmware does **not** call this: its high-priority plane recognises gestures
    /// and feeds them back through [`apply_gesture`](App::apply_gesture), while
    /// [`advance_animations`](App::advance_animations) runs on the map plane. This is exactly those
    /// two halves over `App`'s own [`InputPlane`].
    pub fn handle_input(&mut self, clock: InputClock, input: &mut dyn InputSource) {
        self.now_ms = clock.0;
        // The borrow split is the point: `recognize` borrows `self.input`, so gestures are buffered
        // there and applied *after* it returns (`apply_gesture` touches other fields, never
        // `self.input`). Recognition depends only on the raw events + clock, so this is identical to
        // applying inline; the buffer capacity dwarfs one frame's bounded events.
        let mut pending: heapless::Vec<Gesture, GESTURE_BUF> = heapless::Vec::new();
        self.input.recognize(clock, input, |g| {
            let _ = pending.push(g);
        });
        // A gesture that changes the stack cancels any hold charging at that moment
        // (`apply_gesture` handles the recogniser). A `Hold`/`BackHold` *already recognised into
        // this batch* behind such a transition escaped that cancel — it was aimed at the old top,
        // so drop it here rather than deliver it to the screen that replaced it (issue #480).
        let mut cancelled = false;
        for g in pending {
            if cancelled && matches!(g, Gesture::Hold | Gesture::BackHold) {
                continue;
            }
            self.apply_gesture(g);
            cancelled |= self.take_hold_cancel();
        }
        self.advance_animations(clock);
    }

    /// Drain the pending hold-cancel edge (see `hold_cancel_pending`): `true` when a gesture
    /// changed the screen stack since the last drain, i.e. any hold charging on the host's input
    /// plane is aimed at a vanished target and must be cancelled
    /// ([`InputPlane::cancel_holds`](crate::InputPlane::cancel_holds)). The two-plane firmware
    /// checks this after each drained gesture; [`handle_input`](App::handle_input) consumes it
    /// itself, so single-loop hosts never see it.
    pub fn take_hold_cancel(&mut self) -> bool {
        core::mem::take(&mut self.hold_cancel_pending)
    }

    /// Apply one recognised gesture to the top screen and run the navigation transition it returns —
    /// the **map plane's** half of input handling, split out from recognition. The two-plane
    /// firmware calls this per gesture from its high-priority plane's channel, so the transition
    /// lands a frame after the overlay confirmed the press. Uses the map plane's clock
    /// ([`now_ms`](App::now_ms)) for the [`Ctx`](screen::Ctx).
    pub fn apply_gesture(&mut self, g: Gesture) {
        // Every screen renders into the map plane, so an applied gesture dirties it. Conservative by
        // design (a gesture a screen ignores still costs one redraw), which keeps the idle path
        // exact: with no gesture recognized, `apply_gesture` never runs and the map stays clean.
        self.map_dirty = true;
        // Snapshot the settings so a settings-screen edit is detected by one `==` (Settings is
        // `Copy + Eq`). A change flags a save for the host to pick up via `take_settings_dirty`.
        let settings_before = self.settings;
        let App { state, activity, settings, catalog, ride_catalog, stack, now_ms, poi_scratch, .. } = self;
        let mut cx = Ctx {
            state,
            activity,
            settings,
            routes: catalog.as_slice(),
            rides: ride_catalog.as_slice(),
            poi_scratch,
            now_ms: *now_ms,
        };
        let t = stack.last_mut().expect("the stack always has the Home root").handle(g, &mut cx);
        let depth_before = stack.len();
        // Whether this transition actually changes the stack (Pop/Home at the root are no-ops).
        // A change invalidates any in-flight hold's target — see `hold_cancel_pending`.
        let stack_changed = match &t {
            screen::Transition::None => false,
            screen::Transition::Pop | screen::Transition::Home => depth_before > 1,
            screen::Transition::Push(_) | screen::Transition::Replace(_) | screen::Transition::Root(_) => true,
        };
        screen::apply(stack, t);
        // Opening a POI list drops any previous snapshot so its first draw re-queries at the current
        // fix — the "re-enter to refresh" contract (issue #425). Gated on this being a fresh open
        // (the stack grew), so a turn *within* the list doesn't wipe the frozen snapshot.
        if stack.len() > depth_before && matches!(stack.last(), Some(Screen::PoiList(_))) {
            self.poi_scratch.invalidate();
        }
        // Returning to the bare Home root re-opens the screensaver — re-roll its contour seed so the
        // topo peaks drift for this visit. Gated on the *edge* (was deeper, now 1) so it fires once
        // per return; being in `apply_gesture` means a clock/battery re-render (which never touches
        // the stack) leaves the pattern put.
        if stack.len() == 1 && depth_before > 1 {
            if let Some(Screen::Home(home)) = stack.first_mut() {
                home.reseed(*now_ms);
            }
        }
        // The top screen changed under the rider's finger: cancel any hold charging right now
        // (both `App`'s own recogniser and, via the pending flag, the two-plane firmware's input
        // plane), so a long-press aimed at the *old* top can't complete onto the new one.
        if stack_changed {
            self.input.cancel_holds();
            self.hold_cancel_pending = true;
        }
        if self.settings != settings_before {
            self.settings_dirty = true;
            // A change to the *local* set-point re-stamps the wall clock: the manual clock edit, but
            // also — in GPS mode — a UTC-offset turn or GPS-clock toggle, both of which shift local
            // time. Flipping units or the GPS interval leaves the local clock alone.
            let local_now = self.settings.local_clock();
            if local_now != settings_before.local_clock() {
                self.wall_clock.set(local_now, self.now_ms);
            }
        }
    }

    /// Advance the **map plane's** clock to `clock` and poll each visible screen's timers
    /// ([`Screen::tick_timers`]) in one pass: any time-driven repaint that fired (the Statistics
    /// cursor's spring-back, the Home clock's minute rollover) dirties the map — so a screen
    /// surfaces its own timed-refresh rather than the host re-rendering on a blind heartbeat — and
    /// the soonest residual deadline is stored for [`ms_until_next_wake`](App::ms_until_next_wake).
    /// Cheap: a clock comparison per drawn screen, over the same `base..` range
    /// [`render_map`](App::render_map) draws.
    ///
    /// [`handle_input`](App::handle_input) calls this for the single-loop hosts; the two-plane
    /// firmware calls it directly on its map plane.
    pub fn advance_animations(&mut self, clock: InputClock) {
        self.now_ms = clock.0;
        let now = self.wall_clock.now(self.now_ms);
        let ms_to_next_minute = self.wall_clock.ms_to_next_minute(self.now_ms);
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        let (w, h) = self.frame_size;
        let mut next_wake = None;
        for scr in self.stack.iter_mut().skip(base) {
            let tick = scr.tick_timers(self.now_ms, now, ms_to_next_minute, &self.settings, w, h);
            // A change that promises a containing region accumulates apart from the full-frame
            // demand (#500 follow-up): `take_dirty` folds the two — any `map_dirty` overrides
            // every region, so a region-clipped repaint happens only when region ticks were the
            // *sole* dirt since the last drain.
            if tick.changed {
                match tick.region {
                    Some(r) => self.region_dirty = Some(self.region_dirty.map_or(r, |acc| union_rect(acc, r))),
                    None => self.map_dirty = true,
                }
            }
            next_wake = next_wake.into_iter().chain(tick.next_wake_ms).min();
        }
        self.next_wake_ms = next_wake;
        // The route-upload popups' per-pass reconcile (epic #447, P4): land a hold-deferred
        // prompt, and run the 30 s auto-close (timeout = dismiss). Here — the one hook every host
        // runs each pass — rather than a new timer path; the popups' `tick_timers` above already
        // armed the wake that gets a parked device to this line at the deadline.
        self.reconcile_upload_prompt();
        self.close_expired_upload_popups();
        // Land any warning (issue #504) deferred behind a passkey card / a live hold on an earlier pass.
        self.reconcile_warning();
    }

    /// The single "next wake deadline" the event-driven host arms one timer to: the soonest, in
    /// millis from `now_ms`, that any visible screen needs a *timed* redraw — or `None` when nothing
    /// is time-animating (sleep until an input or sensor event). A read of the deadline
    /// [`advance_animations`](App::advance_animations) stored, so **call it right after
    /// `advance_animations`** in the same frame, with the same `now_ms` (debug-asserted): any *due*
    /// animation has then already fired, so the deadline is strictly in the future.
    pub fn ms_until_next_wake(&self, now_ms: u32) -> Option<u32> {
        debug_assert_eq!(
            now_ms, self.now_ms,
            "ms_until_next_wake must follow advance_animations in the same frame, with the same now_ms"
        );
        self.next_wake_ms
    }

    /// Render the current screen and any overlays above it into `target`, a `w`×`h` pixel display.
    /// Draws from the topmost *opaque* screen upward, so an overlay composites over the still-visible
    /// map. Returns the map [`RenderStats`].
    ///
    /// `color_fn` maps a style's RGB565 to the target's pixel color — the one genuinely
    /// display-specific policy.
    ///
    /// The single-target convenience that draws a whole frame: [`render_map`](App::render_map) then
    /// [`render_overlay`](App::render_overlay) into the *same* target. Hosts that keep the map and
    /// overlay on separate buffers call the two halves directly.
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

    /// Render **only the map plane** — the screen stack from the topmost opaque screen upward, but
    /// **excluding** the global hold-hint chrome. Returns the map [`RenderStats`].
    ///
    /// The expensive half (24–51 ms on the device); a host that keeps the overlay on its own buffer
    /// renders this only when the map changed, then repaints the cheap
    /// [`render_overlay`](App::render_overlay) over it at a higher rate.
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
        // Untimed: `NoopClock` leaves the per-stage `*_us` fields at 0 (the device uses
        // `render_map_timed` with a real clock for the benchmark). Always draws the map, so `Some`.
        self.render_map_timed(target, Some(reader), route, w, h, color_fn, &NoopClock)
    }

    /// Like [`render_map`](App::render_map) but threads `clock` to the Map screen's
    /// [`render_timed`](obc_render::MapRenderer::render_timed), so the returned [`RenderStats`]
    /// carries the map's per-stage timings. The device's render benchmark uses this with its own
    /// microsecond clock. Part of the strippable render-instrumentation seam.
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
        // Record the panel size for the screen ticks' region reporting (`advance_animations`) —
        // the one place every host states its real frame dimensions.
        self.frame_size = (w as i32, h as i32);
        // Drain the one-shot region clip (see `set_render_clip`) — `None` on every normal frame.
        let render_clip = self.render_clip.take();

        // Rebuild the cached elevation profile when the active route changes — it streams every
        // chunk, so it's built once on load, never per frame; clears when no route is loaded.
        if self.activity.active_route != self.profile_route {
            self.profile = route.map(|r| r.elevation_profile());
            self.profile_route = self.activity.active_route;
        }

        // Computed before the field borrow below splits `self`.
        let now = self.wall_clock.now(self.now_ms);
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        // The in-screen confirm fill's hold-progress. Prefer a host-supplied value (the two-plane
        // firmware's separate input plane); fall back to `App`'s own input on the single-loop hosts.
        let hold_progress = self.hold_progress_override.unwrap_or_else(|| self.input.encoder_hold_progress());
        let no_fix = !self.has_live_fix(self.now_ms);
        let App {
            state,
            activity,
            settings,
            catalog,
            ride_catalog,
            renderer,
            stack,
            now_ms,
            profile,
            breadcrumb,
            poi_scratch,
            ..
        } = self;
        let mut rx = Render {
            reader,
            renderer,
            state,
            activity,
            settings,
            routes: catalog.as_slice(),
            rides: ride_catalog.as_slice(),
            route,
            profile: profile.as_ref(),
            breadcrumb: &*breadcrumb,
            poi_scratch,
            w: w as i32,
            h: h as i32,
            now_ms: *now_ms,
            now,
            hold_progress,
            no_fix,
            clock,
            stats: RenderStats::default(),
        };
        // The one Canvas of the frame: every screen draws through it (the base screen — the only
        // possible Map — writes `rx.stats`; the overlays above it leave the stats untouched).
        // A drained region clip makes it reject whole out-of-region primitives — the half of a
        // region-scoped repaint the target's pixel clip can't save (#500 follow-up).
        let mut cv = Canvas::new(target, &color_fn);
        cv.set_clip(render_clip);
        for scr in stack.iter().skip(base) {
            scr.draw(&mut cv, &mut rx);
        }
        rx.stats
    }

    /// Render **only the overlay plane** — the transient always-on-top chrome (the global
    /// long-press hint / confirm bulge), over whatever is already in `target`.
    ///
    /// **Compositing contract** (so this can live on its own buffer/layer): `render_overlay` paints
    /// *only* its own pixels — the hold-bulge strips — and **never** clears the rest of the target.
    /// It must be valid drawn over arbitrary existing content, so a host can repaint it over an
    /// unchanged map without re-running [`render_map`](App::render_map). Poll
    /// [`overlay_active`](App::overlay_active) to decide whether a repaint is needed. The bulge is
    /// opaque `palette::HUD`, so it needs no alpha and reads identically on the 8-colour panel.
    pub fn render_overlay<D, F>(&self, target: &mut D, w: f32, h: f32, color_fn: F)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        self.input.render_overlay(target, w, h, color_fn);
    }

    /// Whether the overlay plane has live content this frame — a hold bulge charging, popping, or
    /// retracting. `false` exactly when [`render_overlay`](App::render_overlay) would draw nothing,
    /// so a host driving the overlay as a separate layer can leave it idle.
    pub fn overlay_active(&self) -> bool {
        self.input.overlay_active()
    }

    /// Drain the repaint demand accumulated since the last call, resetting to [`Dirty::CLEAN`]. The
    /// host calls this **once per frame** after [`tick`](App::tick) +
    /// [`handle_input`](App::handle_input), then renders each plane only when its flag is set — the
    /// render-on-demand loop.
    ///
    /// [`map`](Dirty::map) accumulates every map-affecting mutation since the last drain.
    /// [`overlay`](Dirty::overlay) is *derived* from the live hold-bulge state: set while the bulge
    /// is live, plus one trailing frame after it goes quiet so the host can clear it off Layer 2.
    /// That trailing edge is tracked across calls, so draining twice in one frame swallows it — call
    /// exactly once per frame.
    ///
    /// [`region`](Dirty::region) carries the accumulated region-scoped tick demand — but only when
    /// no full-frame demand joined it since the last drain: a set `map_dirty` covers any region, so
    /// the region folds away and the host full-repaints (over-redraw is safe; under-redraw is a bug).
    pub fn take_dirty(&mut self) -> Dirty {
        let full = core::mem::take(&mut self.map_dirty);
        let region = self.region_dirty.take();
        Dirty {
            map: full || region.is_some(),
            overlay: self.input.take_overlay_dirty(),
            region: if full { None } else { region },
        }
    }

    /// The most recently recognized gesture (host input readout), if any.
    pub fn last_gesture(&self) -> Option<Gesture> {
        self.input.last_gesture()
    }

    /// In-flight encoder hold-progress (0.0–1.0) for the confirm-ring readout.
    pub fn encoder_hold_progress(&self) -> f32 {
        self.input.encoder_hold_progress()
    }

    /// Feed the live encoder hold-progress (0.0–1.0) for the in-screen confirm fills (the factory
    /// Reset bar). The **two-plane firmware** calls this each frame from its high-priority
    /// [`InputPlane`], whose hold state `App`'s own plane doesn't see — without it the Reset bar
    /// never fills. The single-loop hosts never call it (the render reads `App`'s own input). Pairs
    /// with [`base_draws_map`](App::base_draws_map) + [`top_wants_hold_fill`](App::top_wants_hold_fill):
    /// the host forces a redraw while a hold charges on a cheap screen that would draw the fill, so
    /// it animates (a pure hold-charge doesn't otherwise dirty the map).
    pub fn set_hold_progress(&mut self, progress: f32) {
        self.hold_progress_override = Some(progress);
    }

    /// Arm the one-shot region clip for the next [`render_map_timed`](App::render_map_timed) —
    /// the render-side half of a region-scoped repaint (#500 follow-up). The host that drained a
    /// [`Dirty`](crate::Dirty) whose [`region`](crate::Dirty::region) survived calls this with
    /// that region right before rendering; the frame's `Canvas` then skips whole primitives whose
    /// bounds miss it. Pair it with a matching pixel clip on the framebuffer (the two-plane
    /// firmware's `FbDevice64::set_clip`): rejection alone leaves straddling primitives painting
    /// outside the region. Cleared by the render itself; hosts that always repaint fully (the
    /// sim) never call this.
    pub fn set_render_clip(&mut self, clip: Option<Rectangle>) {
        self.render_clip = clip;
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

/// The bounding union of two rects — how `advance_animations` folds multiple region-scoped tick
/// changes into one containing dirty region (embedded-graphics 0.8 has `intersection` but no
/// union). Both operands are screen regions, so non-empty by construction.
fn union_rect(a: Rectangle, b: Rectangle) -> Rectangle {
    use embedded_graphics::prelude::{Point, Size};
    let x0 = a.top_left.x.min(b.top_left.x);
    let y0 = a.top_left.y.min(b.top_left.y);
    let x1 = (a.top_left.x + a.size.width as i32).max(b.top_left.x + b.size.width as i32);
    let y1 = (a.top_left.y + a.size.height as i32).max(b.top_left.y + b.size.height as i32);
    Rectangle::new(Point::new(x0, y0), Size::new((x1 - x0) as u32, (y1 - y0) as u32))
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

    /// A clock source that yields one GPS UTC time then runs dry (one fresh stamp per `tick`).
    struct OneClock(Option<crate::hal::GpsTime>);
    impl crate::hal::ClockSource for OneClock {
        fn poll(&mut self) -> Option<crate::hal::GpsTime> {
            self.0.take()
        }
    }

    fn moving(course: f32) -> Fix {
        Fix { lat: 0, lon: 0, course: Some(course), speed_mps: Some(5.0) }
    }

    /// Tick once with only a GPS clock source (no fix / other sensors), at the map-plane clock
    /// `now_ms` — the timebase `wall_clock_now` reads, set here so the stamp + read agree.
    fn tick_clock(app: &mut App, t: crate::hal::GpsTime, now_ms: u32) {
        app.now_ms = now_ms; // mirror `advance_animations(now)` running right before `tick(now)`
        let mut loc = OneFix(None);
        let mut clock = OneClock(Some(t));
        app.tick(
            RideClock(now_ms),
            Sensors {
                loc: &mut loc,
                altimeter: None,
                temperature: None,
                clock: Some(&mut clock),
                compass: None,
                track: None,
                fuel: None,
            },
            None,
        );
    }

    fn gps_time(hour: u8, minute: u8, second: u8) -> crate::hal::GpsTime {
        crate::hal::GpsTime { utc: DateTime { year: 2026, month: 6, day: 30, hour, minute }, second }
    }

    /// In "Set from GPS" mode a resolved GPS UTC stamps the wall clock (the UTC anchor shifted into
    /// local time by the offset); in manual mode the GPS time is ignored so a hand-set clock is
    /// never overwritten. The set-point updates without flagging a save.
    #[test]
    fn gps_time_sets_the_wall_clock_only_in_gps_mode() {
        // Manual mode: GPS time is ignored; the hand-set clock stands.
        let mut manual = App::new(AppState::new(0, 0, 1.0));
        let hand_set = DateTime { year: 2025, month: 1, day: 1, hour: 9, minute: 0 };
        manual.set_settings(Settings { gps_time: false, clock: hand_set, ..Settings::default() });
        tick_clock(&mut manual, gps_time(14, 37, 0), 1000);
        assert_eq!(manual.wall_clock_now(), hand_set, "manual mode ignores GPS time");
        assert!(!manual.take_settings_dirty(), "a GPS stamp never flags a settings save");

        // GPS mode with a +02:00 offset: local = UTC anchor + offset = 16:37.
        let mut gps = App::new(AppState::new(0, 0, 1.0));
        gps.set_settings(Settings { gps_time: true, utc_offset_min: 120, ..Settings::default() });
        tick_clock(&mut gps, gps_time(14, 37, 0), 1000);
        let now = gps.wall_clock_now();
        assert_eq!((now.hour, now.minute), (16, 37), "GPS UTC 14:37 + 02:00 → local 16:37");
        assert_eq!(gps.settings().clock, gps_time(14, 37, 0).utc, "the stored anchor is the raw UTC");
    }

    /// The seconds-into-the-minute back-date makes the displayed minute roll over at the true
    /// instant, not up to a fix-interval late: a 14:37:56 stamp rolls to 14:38 just 4 s later.
    #[test]
    fn gps_time_back_dates_the_epoch_by_seconds() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.set_settings(Settings { gps_time: true, ..Settings::default() });
        tick_clock(&mut app, gps_time(14, 37, 56), 10_000); // stamped 56 s into the minute
        assert_eq!((app.wall_clock_now().hour, app.wall_clock_now().minute), (14, 37));
        // 4 s on (56 + 4 = 60 s since the minute's true start) the minute must have rolled.
        app.now_ms = 14_000;
        assert_eq!((app.wall_clock_now().hour, app.wall_clock_now().minute), (14, 38), "rolls 4 s later");
        // Without the back-date the same stamp would still read 14:37 here — 4 s isn't a full minute.
    }

    // --- no-GPS-fix freshness + banner edge ---

    /// Tick once with a single fix at the map-plane clock `now_ms` (set so `last_fix_ms` and
    /// `has_live_fix` share a timebase), no route / other sensors.
    fn tick_fix(app: &mut App, fix: Fix, now_ms: u32) {
        app.now_ms = now_ms; // mirror `advance_animations(now)` running right before `tick(now)`
        let mut loc = OneFix(Some(fix));
        app.tick(
            RideClock(now_ms),
            Sensors {
                loc: &mut loc,
                altimeter: None,
                temperature: None,
                clock: None,
                compass: None,
                track: None,
                fuel: None,
            },
            None,
        );
    }

    /// Tick once with no fix at all (the quiet per-frame tick), at the map-plane clock `now_ms`.
    fn tick_idle(app: &mut App, now_ms: u32) {
        app.now_ms = now_ms;
        let mut loc = OneFix(None);
        app.tick(
            RideClock(now_ms),
            Sensors {
                loc: &mut loc,
                altimeter: None,
                temperature: None,
                clock: None,
                compass: None,
                track: None,
                fuel: None,
            },
            None,
        );
    }

    /// `has_live_fix` is `false` before the first fix (acquiring) and once the last fix ages past the
    /// staleness window (lost), and `true` in between — the exact condition the "No GPS Fix" banner
    /// reads. The default 1 s fix interval gives the 5 s floor window.
    #[test]
    fn has_live_fix_tracks_freshness_within_the_window() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        assert!(!app.has_live_fix(0), "no fix yet → not live (acquiring)");

        tick_fix(&mut app, Fix::at(0, 0), 1_000);
        assert!(app.has_live_fix(1_000), "just got a fix → live");
        assert!(app.has_live_fix(1_000 + 5_000), "still live at the window edge");
        assert!(!app.has_live_fix(1_000 + 5_001), "past the window → lost");
    }

    /// The window scales with the configured fix interval, so a long interval doesn't false-trip the
    /// banner in the normal gap between its own (expected) fixes — only when several are missed.
    #[test]
    fn no_fix_window_scales_with_the_fix_interval() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.set_settings(Settings { fix_interval_s: 30, ..Settings::default() }); // window = 30·3 = 90 s
        tick_fix(&mut app, Fix::at(0, 0), 1_000);
        assert!(app.has_live_fix(1_000 + 60_000), "a 60 s gap is within a 30 s-interval window");
        assert!(!app.has_live_fix(1_000 + 90_001), "but past 90 s the fix is lost");
    }

    /// The banner edge is surfaced from the end of `tick` (which runs every frame): a fix aging into
    /// silence dirties the live-data view so the banner appears, and the first/returning fix dirties
    /// it so the banner clears — each exactly once. A stationary returning fix moves the camera
    /// nowhere, so its banner-clear *must* come from this edge, not the fresh-fix redraw path.
    #[test]
    fn no_fix_flip_dirties_the_live_view() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map] → base Map (live data)
        tick_fix(&mut app, Fix::at(0, 0), 1_000); // first fix: banner clears (flip true→false)
        let _ = app.take_dirty();

        tick_idle(&mut app, 3_000);
        assert!(!app.take_dirty().map, "still inside the window → no flip");

        tick_idle(&mut app, 6_001);
        assert!(app.take_dirty().map, "fix went stale → banner appears (map dirtied)");
        tick_idle(&mut app, 7_000);
        assert!(!app.take_dirty().map, "an unchanged no-fix state doesn't re-dirty");

        // A stationary returning fix recenters the camera onto the spot it already sits, so the only
        // thing that changed is the banner — the clear comes from the flip, not a camera move.
        tick_fix(&mut app, Fix::at(0, 0), 20_000);
        assert!(app.take_dirty().map, "fix returned → banner clears (map dirtied)");
    }

    /// The flip never dirties a static Home (it doesn't draw the banner), so a parked idle device
    /// stays clean as a fix ages out — the "static Home does zero renders" criterion still holds.
    #[test]
    fn no_fix_flip_does_not_dirty_idle_home() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0)); // [Home], Idle — not a live-data view
        tick_fix(&mut app, Fix::at(0, 0), 1_000); // flip true→false, but Home isn't a live view
        let _ = app.take_dirty();
        tick_idle(&mut app, 1_000 + 6_001); // the fix ages out → flip false→true, still not live
        assert!(!app.take_dirty().map, "the no-fix flip never dirties a static Home");
    }

    /// A track sink that counts recorded points.
    #[derive(Default)]
    struct CountSink(usize);
    impl crate::hal::TrackSink for CountSink {
        fn record(&mut self, _p: obc_route::TrackPoint) {
            self.0 += 1;
        }
    }

    /// Starting a ride with no fix yet arms the session immediately (Riding, banner up) but records
    /// nothing and books no moving time — then the first fix logs the segment anchor and clears the
    /// banner ("start before lock").
    #[test]
    fn tracking_arms_without_a_fix_and_records_on_first_fix() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.activity.start_session(); // a route load arms a tracking session

        // A tick with no fix: armed, but nothing recorded and no moving time accrued.
        let mut sink = CountSink::default();
        let mut loc = OneFix(None);
        app.now_ms = 1_000;
        app.tick(
            RideClock(1_000),
            Sensors {
                loc: &mut loc,
                altimeter: None,
                temperature: None,
                clock: None,
                compass: None,
                track: Some(&mut sink),
                fuel: None,
            },
            None,
        );
        assert!(app.activity.is_tracking(), "the session is armed immediately, fix or not");
        assert!(!app.has_live_fix(1_000), "no fix yet → the banner is up");
        assert_eq!(sink.0, 0, "nothing recorded while searching");
        assert_eq!(app.activity.moving_s, 0.0, "moving time idles until the first fix");

        // The first fix lands → it's logged (the segment anchor) and the banner clears.
        let mut loc = OneFix(Some(Fix::at(0, 0)));
        app.now_ms = 2_000;
        app.tick(
            RideClock(2_000),
            Sensors {
                loc: &mut loc,
                altimeter: None,
                temperature: None,
                clock: None,
                compass: None,
                track: Some(&mut sink),
                fuel: None,
            },
            None,
        );
        assert!(app.has_live_fix(2_000), "the fix landed → banner clears");
        assert_eq!(sink.0, 1, "the first fix logs the segment anchor");
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
            Sensors {
                loc: &mut loc,
                altimeter: None,
                temperature: None,
                clock: None,
                compass: Some(&mut compass),
                track: None,
                fuel: None,
            },
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

    // --- in-place placement into the reserved region ---

    /// `init_idle` writing field-by-field into a slot must land the same power-on state `new_idle`
    /// builds by value, with the renderer zeroed in place. Guards against a forgotten field.
    #[test]
    fn init_idle_matches_new_idle() {
        use core::mem::MaybeUninit;
        let state = AppState::new(1, 2, 3.0);
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

    // --- end-to-end barometric climb through `tick` ---

    /// Feed one altitude sample through `App::tick`'s `Sensors.altimeter` arm, reading the `climbed`
    /// stat back through the public `App` — the `tick` → `record_altitude` → `climb_m` wiring.
    fn tick_alt(app: &mut App, alt_m: f32, now_ms: u32) {
        let mut loc = OneFix(None); // no fix this tick — isolate the altimeter path
        let mut alt = OneAlt(Some(alt_m));
        app.tick(
            RideClock(now_ms),
            Sensors {
                loc: &mut loc,
                altimeter: Some(&mut alt),
                temperature: None,
                clock: None,
                compass: None,
                track: None,
                fuel: None,
            },
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

    /// The pause rule end-to-end: with the activity paused, `tick` still records the latest altitude
    /// but must not book climb across the rest, so barometer drift while stopped doesn't inflate
    /// `climbed` on resume. Proves the whole tick path honours the mode gate.
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

    /// A settings edit flags a save, but **debounced to leaving the settings subtree**: while still
    /// on a settings screen the pending edit is held (coalescing a multi-detent edit into one
    /// write), surfacing once on the frame after navigating out.
    #[test]
    fn a_settings_edit_flags_dirty_on_leaving_the_settings_subtree() {
        use crate::settings::Units;
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        // Walk to the Units screen (Menu = Routes/POIs/Map/Settings; Settings list = Date&Time/Units/…).
        app.apply_gesture(Gesture::BackHold); // Home → Menu
        app.apply_gesture(Gesture::Turn(-1)); // → Settings entry (wraps back from Routes)
        app.apply_gesture(Gesture::Press); // → Settings list
        app.apply_gesture(Gesture::Turn(1)); // → Units row
        app.apply_gesture(Gesture::Press); // → Units screen
        assert!(!app.take_settings_dirty(), "navigation changed no setting, so nothing to save");

        let before = app.settings().units;
        app.apply_gesture(Gesture::Press); // flip units (live immediately, but persistence is debounced)
        assert_ne!(app.settings().units, before, "the Units screen flipped the system");
        assert_eq!(app.settings().units, Units::Imperial, "default Metric → Imperial");
        assert!(!app.take_settings_dirty(), "still on a settings screen → the save is held, not fired per detent");

        app.apply_gesture(Gesture::Back); // Units → Settings list (still inside the settings subtree)
        assert!(!app.take_settings_dirty(), "the Settings list is itself a settings screen — save stays held");

        app.apply_gesture(Gesture::Back); // Settings list → Menu (left the settings subtree)
        assert!(app.take_settings_dirty(), "leaving settings flushes the pending edit — one coalesced save");
        assert!(!app.take_settings_dirty(), "and the flag drains — only saved once");
    }

    /// Belt-and-braces over [`ScreenKind`](crate::screen::ScreenKind): **every** settings screen
    /// holds a pending save while it is the top screen and flushes it once on exit. Each case
    /// pushes the screen onto the Home root, makes one real edit through the screen's own gestures
    /// where it has one (the Settings list is pure navigation, so its case arms the flag as an
    /// edit made deeper in the subtree would), then backs all the way out. A new settings screen
    /// whose `screens!` row forgets `=> Settings` would flush mid-edit and fail its case here.
    #[test]
    fn every_settings_screen_holds_a_pending_save_until_exit() {
        use crate::screen::{
            apply, AddFieldScreen, DateTimeScreen, PowerScreen, ResetScreen, SettingsScreen, StatFieldsScreen,
            StatsScreen, Transition, UnitsScreen,
        };
        use crate::settings::Units;

        /// The screens to stack on the Home root (bottom first — parents under children, as the
        /// real navigation leaves them) and the gesture script performing one edit on the top one.
        type Case = (&'static str, fn() -> heapless::Vec<Screen, 2>, &'static [Gesture]);
        fn one(s: Screen) -> heapless::Vec<Screen, 2> {
            let mut v = heapless::Vec::new();
            let _ = v.push(s);
            v
        }
        let cases: [Case; 8] = [
            // Pure navigation — no edit gesture of its own.
            ("Settings list", || one(Screen::Settings(SettingsScreen::new())), &[]),
            // Press on row 0 flips the `GPS clock` toggle.
            ("Date & Time", || one(Screen::DateTime(DateTimeScreen::new())), &[Gesture::Press]),
            // Press flips metric ↔ imperial.
            ("Units", || one(Screen::Units(UnitsScreen::new())), &[Gesture::Press]),
            // Open the page-cycle stepper, +1 s (and leave the field open — Back must still exit).
            ("Stats", || one(Screen::Stats(StatsScreen::new())), &[Gesture::Press, Gesture::Turn(1)]),
            // A completed hold deletes the highlighted field.
            ("Fields", || one(Screen::StatFields(StatFieldsScreen::new())), &[Gesture::Hold]),
            // Press adds the highlighted field and pops back onto its Fields parent — still settings.
            (
                "Add field",
                || {
                    let mut v = one(Screen::StatFields(StatFieldsScreen::new()));
                    let _ = v.push(Screen::AddField(AddFieldScreen::new()));
                    v
                },
                &[Gesture::Press],
            ),
            // → the Power Saver row, flip it.
            ("Power", || one(Screen::Power(PowerScreen::new())), &[Gesture::Turn(1), Gesture::Press]),
            // Press arms, then the completed hold erases to defaults — a real diff off the seed below.
            ("Reset", || one(Screen::Reset(ResetScreen::new())), &[Gesture::Press, Gesture::Hold]),
        ];

        for (name, stack, edits) in cases {
            let mut app = App::new_idle(AppState::new(0, 0, 1.0));
            // A non-default seed, so the factory Reset's erase-to-defaults really changes something.
            app.set_settings(Settings { units: Units::Imperial, ..Settings::default() });
            for s in stack() {
                apply(&mut app.stack, Transition::Push(s));
            }
            assert!(app.top_is_settings(), "{name} must classify as ScreenKind::Settings");

            let before = *app.settings();
            for &g in edits {
                app.apply_gesture(g);
            }
            if edits.is_empty() {
                app.settings_dirty = true;
            } else {
                assert_ne!(*app.settings(), before, "{name}: the edit script changed a setting");
            }
            assert!(!app.take_settings_dirty(), "{name}: the save is held while the screen is on top");

            // Back out to the Home root (closing any open field on the way); the save stays held
            // for as long as any settings screen remains on top, then flushes exactly once.
            for _ in 0..MAX_DEPTH_BACKOUT {
                if app.stack.len() == 1 {
                    break;
                }
                assert!(!app.take_settings_dirty(), "{name}: still inside the settings subtree — save held");
                app.apply_gesture(Gesture::Back);
            }
            assert_eq!(app.stack.len(), 1, "{name}: backed out to the Home root");
            assert!(app.take_settings_dirty(), "{name}: leaving the settings subtree flushes the pending save");
            assert!(!app.take_settings_dirty(), "{name}: the flag drains — exactly one save");
        }
    }

    /// Upper bound of `Back` presses needed to unwind any settings case above (open field + the
    /// stacked screens), safely under test control rather than looping forever on a regression.
    const MAX_DEPTH_BACKOUT: usize = 8;

    // --- device warning card (issue #504) ---

    /// The `notify_warning` contract: a raised flag opens the card, further flags coalesce onto the
    /// open one (never a second card), any press dismisses it, and each flag is shown **once** — an
    /// already-shown flag stays quiet, but a genuinely new one re-opens the card with only itself.
    #[test]
    fn warning_card_opens_coalesces_and_shows_each_flag_once() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0)); // [Home]
        assert!(matches!(app.top_screen(), Screen::Home(_)));

        // An empty warning opens nothing.
        app.notify_warning(WarningFlags::NONE);
        assert!(matches!(app.top_screen(), Screen::Home(_)), "an empty warning is a no-op");

        // The first flag opens the card.
        app.notify_warning(WarningFlags::NO_GPS);
        match app.top_screen() {
            Screen::Warning(w) => assert!(w.flags().contains(WarningFlags::NO_GPS)),
            _ => panic!("a raised warning opens the card"),
        }

        // A second flag while the card is up joins it — one card, both flags.
        app.notify_warning(WarningFlags::MAP_SLOW);
        assert_eq!(app.stack.len(), 2, "the new flag joins the open card, not a second one");
        match app.top_screen() {
            Screen::Warning(w) => {
                assert!(w.flags().contains(WarningFlags::NO_GPS));
                assert!(w.flags().contains(WarningFlags::MAP_SLOW));
            }
            _ => panic!("still the one card"),
        }

        // Any press dismisses it back to Home.
        app.apply_gesture(Gesture::Back);
        assert!(matches!(app.top_screen(), Screen::Home(_)), "dismiss pops the card");

        // A flag already shown doesn't nag again.
        app.notify_warning(WarningFlags::NO_GPS);
        assert!(matches!(app.top_screen(), Screen::Home(_)), "an already-shown flag stays quiet");

        // A brand-new flag re-opens the card — showing only the fresh flag, not the acknowledged ones.
        app.notify_warning(WarningFlags::NO_COMPASS);
        match app.top_screen() {
            Screen::Warning(w) => {
                assert!(w.flags().contains(WarningFlags::NO_COMPASS));
                assert!(!w.flags().contains(WarningFlags::NO_GPS), "the re-opened card carries only the new flag");
            }
            _ => panic!("a new flag re-opens the card"),
        }
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

    // --- the live wall clock ---

    /// Seeding the persisted clock stamps the wall clock, which then advances with the monotonic
    /// millis — the static set-point actually ticks (carrying minute → hour here).
    #[test]
    fn wall_clock_advances_from_the_seeded_setpoint() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let seeded = crate::settings::Settings {
            clock: DateTime { year: 2026, month: 6, day: 29, hour: 14, minute: 40 },
            ..Default::default()
        };
        app.set_settings(seeded); // stamps the wall clock at now_ms = 0
        assert_eq!(app.wall_clock_now(), seeded.clock, "at the boot stamp it reads the set-point");
        app.now_ms = 25 * 60_000; // 25 minutes of monotonic time later
        let now = app.wall_clock_now();
        assert_eq!((now.hour, now.minute), (15, 5), "the clock advanced 25 min, carrying into the hour");
    }

    /// Editing the time on the Date & Time screen re-stamps the wall clock, so it resumes ticking
    /// from the freshly set value rather than carrying the pre-edit monotonic offset into it.
    /// Drives the real navigation (Home → Menu → Settings → Date & Time → TIME → minute).
    #[test]
    fn editing_the_clock_restamps_the_wall_clock() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        // Ten minutes of monotonic time since boot: with a stale epoch the display would read the
        // set-point + 10 min, so the re-stamp is exactly what makes it read the edited value.
        app.now_ms = 10 * 60_000;
        app.apply_gesture(Gesture::BackHold); // Home → Menu
        app.apply_gesture(Gesture::Turn(-1)); // → Settings entry (wraps back from Routes)
        app.apply_gesture(Gesture::Press); // → Settings list (row 0 = Date & Time)
        app.apply_gesture(Gesture::Press); // → Date & Time
        app.apply_gesture(Gesture::Turn(2)); // Toggle → DATE → TIME row
        app.apply_gesture(Gesture::Press); // open the hour field
        app.apply_gesture(Gesture::Press); // step to the minute field
        let before = app.settings().clock.minute;
        app.apply_gesture(Gesture::Turn(1)); // minute + 1 → a real clock edit
        let edited = app.settings().clock;
        assert_ne!(edited.minute, before, "the edit moved the minute");
        assert_eq!(app.wall_clock_now(), edited, "the edit re-stamped the clock to the new set-point");
        app.now_ms += 60_000;
        assert_eq!(app.wall_clock_now().minute, (edited.minute + 1) % 60, "ticks on from the new stamp");
    }

    /// In GPS mode the Home wall clock shows **local** time (the UTC anchor shifted by the offset),
    /// so it agrees with the Date & Time screen's "Local time" row instead of trailing it.
    #[test]
    fn gps_mode_wall_clock_shows_local_time() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let seeded = crate::settings::Settings {
            gps_time: true,
            clock: DateTime { year: 2026, month: 6, day: 29, hour: 12, minute: 0 }, // the UTC anchor
            utc_offset_min: 120,                                                    // +02:00
            ..Default::default()
        };
        app.set_settings(seeded);
        let now = app.wall_clock_now();
        assert_eq!((now.hour, now.minute), (14, 0), "Home shows local = UTC + offset, not the raw UTC anchor");
        assert_eq!(now, seeded.local_clock(), "and it matches the local_clock the Local time row reads");
    }

    /// On Home, `advance_animations` self-dirties exactly once per minute as the wall clock rolls
    /// over — the timed repaint that makes the static `HH:MM` advance — and nothing in between.
    #[test]
    fn home_self_dirties_once_a_minute() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0)); // base = Home
        app.set_settings(crate::settings::Settings {
            clock: DateTime { year: 2025, month: 1, day: 1, hour: 12, minute: 0 },
            ..Default::default()
        });
        let _ = app.take_dirty(); // clear the boot-dirty so we observe only the clock's effect
        app.advance_animations(InputClock(0));
        assert!(
            !app.take_dirty().map,
            "the first frame only initialises the ticker — the boot paint already showed the clock"
        );
        app.advance_animations(InputClock(30_000));
        assert!(!app.take_dirty().map, "mid-minute the clock is unchanged — no repaint");
        app.advance_animations(InputClock(60_000));
        assert!(app.take_dirty().map, "the minute rolled over → exactly one repaint");
        app.advance_animations(InputClock(90_000));
        assert!(!app.take_dirty().map, "and it settles back to quiet until the next minute");
    }

    /// `ms_until_next_wake` reports the soonest timed-redraw deadline across the visible stack. On
    /// Home it's the wall-clock minute boundary; on a static menu it's `None` (sleep until input).
    #[test]
    fn ms_until_next_wake_reports_the_home_minute_then_none_on_a_static_menu() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0)); // base = Home
        app.set_settings(crate::settings::Settings {
            clock: DateTime { year: 2025, month: 1, day: 1, hour: 12, minute: 0 },
            ..Default::default()
        });
        // Home shows a clock → the deadline is the time left until the displayed minute rolls over.
        app.advance_animations(InputClock(0));
        assert_eq!(app.ms_until_next_wake(0), Some(60_000), "at a boundary the whole minute remains");
        app.advance_animations(InputClock(25_000));
        assert_eq!(app.ms_until_next_wake(25_000), Some(35_000), "25 s in, 35 s until the next repaint");
        // Navigate to the static Menu (BackHold): it animates on nothing, so there is no deadline —
        // the host sleeps until the next input or sensor event. The same-frame `advance_animations`
        // re-polls the now-visible stack, per `ms_until_next_wake`'s documented contract.
        app.apply_gesture(Gesture::BackHold);
        app.advance_animations(InputClock(25_000));
        assert_eq!(app.ms_until_next_wake(25_000), None, "a static menu needs no timed wake");
    }
}
