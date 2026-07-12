//! [`AppState`] — the device's view state — and [`App`], the shared per-frame
//! driver that both hosts run.

use embedded_graphics::{draw_target::DrawTarget, primitives::Rectangle};
use obc_reader::Reader;
use obc_render::{zoom_for_mpp, Canvas, Clock, MapRenderer, NoopClock, RenderStats, Viewport};
use obc_route::{ClimbProfile, Climbs, Profile, RouteMatch, RouteReader, TrackPoint, Waypoints};

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
///         hr: None,
///         power: None,
///         cadence: None,
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
    /// The loaded map's routing-profile **names** (routing-v2 N5), refreshed by the host on map load
    /// ([`set_nav_profiles`](App::set_nav_profiles)) — resident because the Bike-type settings screen
    /// and the created-route overview label render them on frames the host draws without a `Reader`.
    /// Only the names are mirrored (≤ 8 × 12 B); the multiplier tables stay solely in `MapTables`.
    nav_profiles: crate::NavProfiles,
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
    /// The **viewed ride's** recorded-track elevation profile (epic #678 T2 / #680) — the Ride
    /// detail's band source, and the app's **single** resident ride-profile buffer (the same
    /// [`Profile`] type the route band uses; statics grow by exactly this one buffer). The app
    /// can't build it itself — the track lives on the card — so the host fills it: on detail
    /// entry [`take_ride_track_request`](App::take_ride_track_request) hands out the ride's
    /// durable id, the host streams `RD{id}.ORD` once, and
    /// [`set_ride_profile`](App::set_ride_profile) parks the result here. Invalidated when
    /// [`Activity::viewed_ride`] leaves the ride (exit, or a rescan that vanished it).
    ride_profile: Option<Profile>,
    /// The [`viewed_ride`](Activity::viewed_ride) the [`ride_profile`](App::ride_profile) buffer
    /// was **answered** for (a failed fill parks `None` under the same key, so a dead file isn't
    /// re-streamed every pass), remapped by identity across rescans like every held ride index.
    ride_profile_for: Option<usize>,
    /// The viewed ride's decimated recorded-track shape polyline (#678 rework 3 — the Ride
    /// detail's track pager page), ≤ [`NAV_PREVIEW_MAX`] `(lon, lat)` µdeg points, host-filled via
    /// [`set_ride_preview`](App::set_ride_preview) in the same drain as the ride profile.
    ride_preview: heapless::Vec<(i32, i32), NAV_PREVIEW_MAX>,
    /// The [`viewed_ride`](Activity::viewed_ride) the [`ride_preview`](App::ride_preview) was
    /// handed in for — the staleness key (render gates on it; the exit/rescan invalidation drops
    /// it alongside the profile), remapped by identity across rescans like the profile key.
    ride_preview_for: Option<usize>,
    /// The Route overview's decimated route-shape preview polyline (#685 §4, generalized to
    /// stored routes by #678 rework 3's content-paired pager) — ≤ [`NAV_PREVIEW_MAX`]
    /// `(lon, lat)` µdeg points, **decimated host-side** and handed in via
    /// [`set_nav_preview`](App::set_nav_preview): a computed route's at plan-commit time, a
    /// stored route's on overview entry (the per-pass [`nav_preview_missing`](App::nav_preview_missing)
    /// cue). Drawn by the Route overview; empty otherwise.
    nav_preview: heapless::Vec<(i32, i32), NAV_PREVIEW_MAX>,
    /// The [`active_route`](Activity::active_route) the [`nav_preview`](App::nav_preview) was
    /// handed in for — the staleness key ([`nav_preview_missing`](App::nav_preview_missing)
    /// compares it, and the render gates on it so an old plan's shape can never draw under a
    /// different route). Cleared by [`notify_nav_result`](App::notify_nav_result) so every plan
    /// starts preview-less.
    nav_preview_route: Option<usize>,
    /// The active route's detected climbs, segmented once on route load (one streaming chunk sweep,
    /// so never per frame). Empty when no route is loaded; [`climbs_route`](App::climbs_route)
    /// tracks which route the list was built for. The riding views query it (with hysteresis, via
    /// [`update_active_climb`](App::update_active_climb)) to decide "am I on a climb now?".
    climbs: Climbs,
    /// The [`active_route`](Activity::active_route) the cached [`climbs`](App::climbs) list was
    /// built for, so a route change triggers exactly one re-segmentation. Kept apart from
    /// [`profile_route`](App::profile_route) even though they change together, so each cache states
    /// its own build key.
    climbs_route: Option<usize>,
    /// The active route's resident named-waypoint table, loaded once on route load (it streams the
    /// stored waypoint section, so never per frame) — the waypoint twin of [`climbs`](App::climbs).
    /// Empty when no route is loaded; the riding views read it (map diamonds, the approach chip, the
    /// progress-bar ticks, the stat fields — later in the epic) and [`Activity::next_waypoint`]
    /// indexes it. A [`truncated`](obc_route::Waypoints::truncated) table is re-windowed forward in
    /// [`tick`](App::tick) once the rider passes its tail.
    waypoints: Waypoints,
    /// The [`active_route`](Activity::active_route) the cached [`waypoints`](App::waypoints) table was
    /// loaded for — its own build key, alongside [`climbs_route`](App::climbs_route). A re-window
    /// leaves it pointed at the same route (it reloads a *later* window of the same route, not a new
    /// route), so only an actual route change reloads from the start.
    waypoints_route: Option<usize>,
    /// The **single** resident detail profile for the currently-active climb — one buffer refilled
    /// in place only when [`Activity::active_climb`] transitions to a new `Some(i)`, never per frame
    /// (the fill streams the climb's chunks; ~400 B, held resident to keep it off the ~36 KB device
    /// stack). Meaningless (a flat base line) while no climb is active; the [`Render`] surface only
    /// hands it out alongside a `Some` `active_climb`, so a stale buffer is never drawn.
    climb_profile: ClimbProfile,
    /// Test-only tally of [`ClimbProfile::fill`] calls, so a test can assert the detail buffer is
    /// rebuilt **exactly** on climb-entry transitions — never per fix on the same climb. Not
    /// compiled into the firmware.
    #[cfg(test)]
    climb_fill_count: u32,
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
    /// Map-plane millis of the last **user input** — any recognised gesture (see
    /// [`apply_gesture`](App::apply_gesture)), plus a per-tick refresh while a hold charges (a
    /// gesture in progress counts as activity). Drives the **idle-return** timeout
    /// ([`apply_idle_return`](App::apply_idle_return)): after
    /// [`idle_return`](crate::settings::Settings::idle_return) millis of silence the UI navigates
    /// itself back to where it belongs. Deliberately advanced **only** on input — a GPS fix, a BLE
    /// event, or a timed repaint must not reset it. Seeded to `0` (the boot origin), so the idle
    /// clock runs from power-on until the first touch.
    last_input_ms: u32,
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
    /// at a popup's "Finish & new" must never land on the Route menu's hold-to-delete footer —
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
    /// The sensor-tile display values `(hr, power, cadence)` at the previous `tick`'s end, so a
    /// fresh BLE sample — or the 5 s staleness gate expiring one into `--` — repaints the riding
    /// views exactly once (the sensor twin of [`prev_no_fix`](App::prev_no_fix)). The samples land
    /// in [`Activity`], which the `state != state_before` redraw gate never compares, so without
    /// this edge a live tile only repainted when something *else* (a moving fix, a screen change)
    /// happened to dirty the frame — frozen solid on an indoor bench with no fix (epic #744, SR3).
    prev_live_sensors: (Option<u16>, Option<u16>, Option<u8>),
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
    /// The per-slot BLE **sensor status** (BLE sensors epic #707, SE7): HR / power / cadence
    /// connection phase + battery + live tick, fed each pass by the host through
    /// [`set_sensor_status`](App::set_sensor_status) and drawn only by the Sensors settings screen.
    /// Held off [`AppState`] like [`ble_passkey`](App::ble_passkey) so feeding it never gates a map
    /// redraw on a non-sensor screen; the Sensors screen's repaint is gated on an actual change to a
    /// slot while it is up.
    sensor_status: [crate::sensors::SensorStatus; crate::settings::SENSOR_SLOTS],
    /// The live **sensor scan hits** (SE7): the sensors discovered while the scan-list screen runs a
    /// scan, fed by the host through [`set_sensor_scan_hits`](App::set_sensor_scan_hits). Empty
    /// outside a scan; replaced wholesale each pass while one runs.
    sensor_scan_hits: crate::sensors::SensorScanHits,
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
    /// The firmware update **this boot just confirmed** (S4, #619): the running image's version
    /// string, set by the board once the trial confirm has written `Idle { installed }`. The
    /// one-time fact S5's "updated to vX" toast takes; `None` on a normal boot.
    update_confirmed: Option<heapless::String<32>>,
    /// The firmware update **this boot detected as failed** (the board's boot-outcome reconcile):
    /// the typed [`DfuFailure`](crate::dfu::DfuFailure) verdict + the staged version the arm
    /// marker recorded (if it survived). The one-time fact the "UPDATE FAILED" card takes; `None`
    /// on a normal boot.
    update_failed: Option<(crate::dfu::DfuFailure, Option<heapless::String<32>>)>,
    /// The running firmware version string (T8 item 6) — the same value the DFU confirm shows as
    /// "Installed", fed by the host at boot via [`set_fw_version`](App::set_fw_version). The System
    /// settings screen's `Firmware` ledger row renders it (empty ⇒ `--`). Resident because that frame
    /// draws without a `Reader`, like [`nav_profiles`](App::nav_profiles).
    fw_version: heapless::String<32>,
    /// The loaded map's display name (T8 item 6), fed on map load via
    /// [`set_map_info`](App::set_map_info) — the left half of the System screen's `Map` row
    /// (`grimsel · v10`). Empty until a map loads. Resident (the frame draws without a `Reader`).
    map_name: heapless::String<24>,
    /// The loaded map's OBCM format version, the right half of the `Map` row. `0` until a map loads.
    map_obcm_version: u8,
    /// Free space on the SD card in bytes (T8 item 6), answered by the host's FAT free-cluster scan
    /// after the System screen posts its one-shot on entry
    /// ([`take_card_scan_request`](App::take_card_scan_request) → [`set_card_free`](App::set_card_free)).
    /// `None` until the host answers — the screen shows `--`.
    card_free_bytes: Option<u64>,
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
    /// The route's mini elevation sparkline ([`obc_route::elevation_sparkline`]), built by the host
    /// from the just-committed OBCR at commit time (#682) — `None` when the route carries no
    /// elevation. Carried with the event so the idle "ROUTE RECEIVED" card can draw it; the
    /// mid-ride swap / active-replace variants ignore it.
    elevation: Option<[u8; obc_route::SPARKLINE_BUCKETS]>,
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

/// Cap on the computed route's shape-preview polyline (#685 §4): the host decimates the planned
/// polyline to at most this many points before handing it to
/// [`set_nav_preview`](App::set_nav_preview) — plenty for the overview's ~212×90 px sketch, and a
/// fixed ~512 B resident buffer here rather than a route-sized one.
pub const NAV_PREVIEW_MAX: usize = 64;

/// Enter/exit hysteresis for [`App::update_active_climb`] — the margins that turn the raw interval
/// lookup ([`Climbs::active_at`], exact detected geometry, no slack) into a flap-free "on a climb
/// now" state.
///
/// **Enter early, exit late.** The raw intervals are the detected trough→summit, but the matched
/// `progress_m` jitters a few metres either way of the true position each fix (matcher snap +
/// smoothing). Without slack a rider straddling the base or the summit would toggle the Climb
/// screen on and off between consecutive fixes. So we **arm** the climb once progress reaches
/// [`CLIMB_ENTER_MARGIN_M`] *before* the base and **hold** it until progress passes
/// [`CLIMB_EXIT_MARGIN_M`] *past* the summit — an on-then-off band wider than the jitter, biased so
/// the panel appears slightly ahead of the ramp (useful) and lingers slightly past the crest
/// (avoids a premature dismissal on the false-flat over the top).
///
/// The margins are asymmetric on purpose: showing the climb a touch early is welcome, and holding a
/// touch past the crest reads better than snapping away the instant `progress == end_m`. Both are
/// well under [`obc_route::MIN_LEN`] (400 m), so they can't make one climb's exit band overlap the
/// next climb's entry band on any kept climb.
const CLIMB_ENTER_MARGIN_M: u32 = 50;
/// Distance (m) past a climb's summit the "on climb" state is held before it disarms — see
/// [`CLIMB_ENTER_MARGIN_M`].
const CLIMB_EXIT_MARGIN_M: u32 = 30;

/// The active-climb hysteresis, as a **pure** function of the climbs list, the matched progress,
/// and the previous active index — the whole flap-guard policy in one testable place (the
/// `App::update_active_climb` wrapper only adds the off-route freeze and the once-per-entry refill).
///
/// While *on* climb `prev`, hold it until `progress` passes its summit + [`CLIMB_EXIT_MARGIN_M`]
/// (or the index went stale — a shrunk list after a swap); otherwise re-arm. To *arm* a climb,
/// `progress` must have reached within [`CLIMB_ENTER_MARGIN_M`] of its base and not yet passed its
/// summit — the first such climb in route order (they're non-overlapping and the margins are far
/// under [`obc_route::MIN_LEN`], so the bands can't collide on kept climbs). The exit band is wider
/// on the far side and the entry band on the near side, so a rider straddling a boundary can't
/// toggle the state between consecutive fixes.
fn resolve_active_climb(climbs: &Climbs, progress: u32, prev: Option<usize>) -> Option<usize> {
    // While committed to a climb, hold it across its exit band before reconsidering.
    if let Some(i) = prev {
        if let Some(seg) = climbs.as_slice().get(i) {
            if progress <= seg.end_m.saturating_add(CLIMB_EXIT_MARGIN_M) {
                return Some(i);
            }
        }
    }
    // Not held: arm the first climb whose entry band (base − enter margin ..= summit) contains
    // progress.
    climbs
        .as_slice()
        .iter()
        .position(|c| progress >= c.start_m.saturating_sub(CLIMB_ENTER_MARGIN_M) && progress <= c.end_m)
}

/// Distance (m) a passed waypoint **lingers** as "next" before the index advances — distance
/// hysteresis, not time. GPS jitter around a waypoint's position stays inside this band, so the
/// resolved index can't flap there; the shown distance-to-go clamps to 0 through the linger. Matches
/// the epic's 100 m pass-linger.
pub(crate) const WAYPOINT_LINGER_M: u32 = 100;

/// The next-waypoint index as a **pure** function of the resident table, the matched progress, and
/// the previously-resolved index — the waypoint sibling of [`resolve_active_climb`]. The
/// [`update_next_waypoint`](App::update_next_waypoint) wrapper adds the off-route freeze and the
/// re-window; this is the whole "which waypoint is next?" policy in one testable place.
///
/// The next waypoint is the **first entry still ahead**: one whose linger band is open,
/// `progress < dist_along_m + WAYPOINT_LINGER_M`. A passed waypoint therefore lingers
/// [`WAYPOINT_LINGER_M`] before the index moves on, so jitter around a waypoint can't flap it. `prev`
/// only keeps the index from *regressing* on a progress dip (jitter at the far, advance edge of the
/// band) — it never steps back onto a waypoint already passed while progress oscillates. `None` once
/// the rider is past every waypoint's linger.
fn resolve_next_waypoint(wpts: &Waypoints, progress_m: u32, prev: Option<usize>) -> Option<usize> {
    let ahead = wpts.as_slice().iter().position(|w| progress_m < w.dist_along_m.saturating_add(WAYPOINT_LINGER_M));
    match ahead {
        // Past every waypoint's linger — the chip / fields go empty even if one was held.
        None => None,
        // Hold the furthest-reached index against a jittering cursor (never un-pass a waypoint);
        // otherwise take the first still-ahead one. A stale `prev` (≥ len, after a table shrink)
        // falls through to `a`.
        Some(a) => match prev {
            Some(p) if p > a && p < wpts.len() => Some(p),
            _ => Some(a),
        },
    }
}

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
    /// Idle, no route loaded. Loading a route (Home → Menu → Routes → `press`) starts
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
            nav_profiles: crate::NavProfiles::new(),
            stack,
            profile: None,
            profile_route: None,
            ride_profile: None,
            ride_profile_for: None,
            ride_preview: heapless::Vec::new(),
            ride_preview_for: None,
            nav_preview: heapless::Vec::new(),
            nav_preview_route: None,
            climbs: Climbs::new(),
            climbs_route: None,
            waypoints: Waypoints::new(),
            waypoints_route: None,
            climb_profile: ClimbProfile::new(),
            #[cfg(test)]
            climb_fill_count: 0,
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
            last_input_ms: 0,
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
            prev_live_sensors: (None, None, None),
            poi_scratch: screen::PoiScratch::new(),
            ble_passkey: None,
            sensor_status: [crate::sensors::SensorStatus::default(); crate::settings::SENSOR_SLOTS],
            sensor_scan_hits: crate::sensors::SensorScanHits::new(),
            store_changed_pending: 0,
            pending_upload: None,
            pending_warnings: WarningFlags::NONE,
            warned: WarningFlags::NONE,
            update_confirmed: None,
            update_failed: None,
            fw_version: heapless::String::new(),
            map_name: heapless::String::new(),
            map_obcm_version: 0,
            card_free_bytes: None,
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
            // (Was missing until #678 rework 3's field audit, like #680's `update_failed` catch:
            // the profile-name mirror and the two shape-preview fields must be initialized like
            // every other, or the board's first render reads uninit memory through them.)
            addr_of_mut!((*slot).nav_profiles).write(crate::NavProfiles::new());
            // The screen stack: empty in place, then push the always-present Home root.
            // `heapless::Vec::push` isn't `const`, so the root can't be part of a literal.
            addr_of_mut!((*slot).stack).write(Stack::new());
            let _ = (*slot).stack.push(Screen::Home(HomeScreen::new()));
            addr_of_mut!((*slot).profile).write(None);
            addr_of_mut!((*slot).profile_route).write(None);
            addr_of_mut!((*slot).ride_profile).write(None);
            addr_of_mut!((*slot).ride_profile_for).write(None);
            addr_of_mut!((*slot).ride_preview).write(heapless::Vec::new());
            addr_of_mut!((*slot).ride_preview_for).write(None);
            addr_of_mut!((*slot).nav_preview).write(heapless::Vec::new());
            addr_of_mut!((*slot).nav_preview_route).write(None);
            // The climb caches mirror the profile: an empty list + a zeroed detail buffer
            // (`Climbs::new`/`ClimbProfile::new` are const, so no large temporary is formed here).
            addr_of_mut!((*slot).climbs).write(Climbs::new());
            addr_of_mut!((*slot).climbs_route).write(None);
            // The waypoint table mirrors the climbs list: an empty table (~1.3 KB) written straight
            // into the slot, keyed to no route until the first load.
            addr_of_mut!((*slot).waypoints).write(Waypoints::new());
            addr_of_mut!((*slot).waypoints_route).write(None);
            addr_of_mut!((*slot).climb_profile).write(ClimbProfile::new());
            #[cfg(test)]
            addr_of_mut!((*slot).climb_fill_count).write(0);
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
            addr_of_mut!((*slot).last_input_ms).write(0);
            addr_of_mut!((*slot).settings).write(Settings::default());
            addr_of_mut!((*slot).wall_clock).write(WallClock::new(Settings::default().local_clock()));
            addr_of_mut!((*slot).settings_dirty).write(false);
            addr_of_mut!((*slot).hold_progress_override).write(None);
            addr_of_mut!((*slot).hold_cancel_pending).write(false);
            addr_of_mut!((*slot).last_battery_poll_ms).write(None);
            addr_of_mut!((*slot).temp_c).write(None);
            addr_of_mut!((*slot).last_fix_ms).write(None);
            addr_of_mut!((*slot).prev_no_fix).write(true);
            addr_of_mut!((*slot).prev_live_sensors).write((None, None, None));
            addr_of_mut!((*slot).poi_scratch).write(screen::PoiScratch::new());
            addr_of_mut!((*slot).ble_passkey).write(None);
            addr_of_mut!((*slot).sensor_status)
                .write([crate::sensors::SensorStatus::default(); crate::settings::SENSOR_SLOTS]);
            addr_of_mut!((*slot).sensor_scan_hits).write(crate::sensors::SensorScanHits::new());
            addr_of_mut!((*slot).store_changed_pending).write(0);
            addr_of_mut!((*slot).pending_upload).write(None);
            addr_of_mut!((*slot).pending_warnings).write(WarningFlags::NONE);
            addr_of_mut!((*slot).warned).write(WarningFlags::NONE);
            addr_of_mut!((*slot).update_confirmed).write(None);
            // (Was missing until #680's field audit: the boot-verdict field must be initialized
            // like every other, or the board's first `reconcile_update_toast` reads uninit memory.)
            addr_of_mut!((*slot).update_failed).write(None);
            addr_of_mut!((*slot).fw_version).write(heapless::String::new());
            addr_of_mut!((*slot).map_name).write(heapless::String::new());
            addr_of_mut!((*slot).map_obcm_version).write(0);
            addr_of_mut!((*slot).card_free_bytes).write(None);
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
        // BLE-sensor freshness is judged on the `RideClock` (`now_ms`) — the clock samples record on
        // and the summaries + track log read on. Remember it so the stat tiles, which render *after*
        // this tick against the map-plane clock `self.now_ms`, judge staleness on the same timebase.
        // On the board `self.now_ms == now_ms` (the ride loop drives `advance_animations` and `tick`
        // off one monotonic `now`); in the simulator they differ (`RideClock` is GPX-playback time,
        // `self.now_ms` is wall time), and a tile reading `self.now_ms` would blank to `--` seconds
        // into a replay — see the `sensor_tiles_…` test.
        self.activity.note_sensor_clock(now_ms);
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

        // Segment the route's climbs once per load — the twin of the elevation-profile rebuild, but
        // done here in `tick` (not render) because `update_active_climb` below needs the list before
        // the fix is matched. Like the profile it streams every chunk, so it's built once and never
        // per frame. Only advance the build key when the geometry is actually streamable: a `None`
        // route (idle, or a transient SD glitch) leaves the empty list in place and retries next
        // tick, rather than latching an empty result for the route.
        if self.activity.active_route != self.climbs_route {
            match (self.activity.active_route, route) {
                (Some(_), Some(r)) => {
                    self.climbs = r.detect_climbs();
                    self.climbs_route = self.activity.active_route;
                    self.activity.active_climb = None; // a fresh list — re-derive the active climb below
                }
                (None, _) => {
                    // The route unloaded: drop the climbs and the on-climb state.
                    self.climbs = Climbs::new();
                    self.climbs_route = None;
                    self.activity.active_climb = None;
                }
                (Some(_), None) => { /* geometry not yet streamable — keep the old state, retry next tick */ }
            }
        }

        // Load the route's named waypoints once per load, alongside the climbs above and on the same
        // streamable-geometry guard — the resident table the riding views (and `resolve_next_waypoint`
        // below) read. Loaded from the route start (`min_dist_m = 0`); a truncated table is slid
        // forward later, in `update_next_waypoint`, not here.
        if self.activity.active_route != self.waypoints_route {
            match (self.activity.active_route, route) {
                (Some(_), Some(r)) => {
                    self.waypoints = r.load_waypoints(0);
                    self.waypoints_route = self.activity.active_route;
                    self.activity.next_waypoint = None; // a fresh table — re-derive the next waypoint below
                }
                (None, _) => {
                    // The route unloaded: drop the table and the next-waypoint state.
                    self.waypoints = Waypoints::new();
                    self.waypoints_route = None;
                    self.activity.next_waypoint = None;
                }
                (Some(_), None) => { /* geometry not yet streamable — keep the old table, retry next tick */ }
            }
        }

        let Sensors { loc, altimeter, temperature, clock, compass, track, fuel, hr, power, cadence } = sensors;
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
        // BLE sensors → the live values Activity staleness-gates + the per-ride summaries. Drained
        // here beside the altimeter/temperature so `record_motion` (below, on a fresh fix) sees this
        // tick's samples. `Some` only on a fresh reading; a dropped strap simply stops reporting and
        // the staleness gate expires the last value. The stat tiles (SE5) read these through the
        // `live_*_display` accessors; their repaint edge is the `prev_live_sensors` comparison at
        // the end of this tick.
        if let Some(hr) = hr {
            if let Some(bpm) = hr.poll() {
                self.activity.record_hr(bpm, now_ms);
            }
        }
        if let Some(power) = power {
            if let Some(watts) = power.poll() {
                self.activity.record_power(watts, now_ms);
            }
        }
        if let Some(cadence) = cadence {
            if let Some(rpm) = cadence.poll() {
                self.activity.record_cadence(rpm, now_ms);
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
                // "Am I on a climb now?" is derived from the fresh match — with hysteresis, and a
                // detail-profile refill only on a new climb entry (see `update_active_climb`).
                self.update_active_climb(route);
                // "Which waypoint is next?" from the same fresh progress — distance-lingered, and it
                // re-windows a truncated table forward as the rider advances (see below).
                self.update_next_waypoint(route);
            }
            let motion = self.activity.record_motion(fix, now_ms);
            if motion.log {
                self.breadcrumb.push(fix.lon, fix.lat);
                if let Some(track) = track {
                    let logged = track.record(TrackPoint {
                        lon: fix.lon,
                        lat: fix.lat,
                        ele: self.activity.track_ele(),
                        t_ms: now_ms,
                        segment_start: motion.segment_start,
                        // Stamp the freshest staleness-gated sensor values (epic #707): a strap
                        // that's dropped/stale (>5 s) records absent, never its frozen last value.
                        // `now_ms` (the RideClock) is the same timebase the samples arrived on.
                        hr: self.activity.live_hr(now_ms).map(|b| b.min(u8::MAX as u16) as u8),
                        cadence: self.activity.live_cadence(now_ms),
                        power: self.activity.live_power(now_ms),
                    });
                    // The host couldn't durably write the point (card pulled, write error, medium
                    // full) — the ride log now has a gap. Raise the recording-error advisory so the
                    // rider isn't left thinking the ride is being logged when it isn't (issue #11).
                    // `notify_warning` latches it once per boot, so a whole ride of failing writes
                    // raises one dismissable card, not a per-fix nag.
                    if logged.is_err() {
                        self.notify_warning(WarningFlags::REC_ERROR);
                    }
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
        // A live sensor tile's displayed value changed — a fresh BLE sample, or the 5 s staleness
        // gate expiring one into `--`. Like the no-fix banner, this is an edge off data the
        // `AppState` comparison never sees (the samples live in `Activity`), surfaced at the end of
        // `tick` so it compares the exact values the render will draw (epic #744, SR3). Gated per
        // quantity on the field actually being pinned to the grid, so an unconfigured sensor never
        // forces a full map render at its notification rate (the same economy as the battery /
        // `temp_c` gates above).
        {
            use crate::stat_fields::StatField;
            let live = (
                self.activity.live_hr_display(),
                self.activity.live_power_display(),
                self.activity.live_cadence_display(),
            );
            if live != self.prev_live_sensors {
                let fields = &self.settings.stat_fields;
                let shown = (live.0 != self.prev_live_sensors.0 && fields.contains(StatField::HeartRate))
                    || (live.1 != self.prev_live_sensors.1 && fields.contains(StatField::Power))
                    || (live.2 != self.prev_live_sensors.2 && fields.contains(StatField::Cadence));
                if shown && self.shows_live_data() {
                    self.map_dirty = true;
                }
                self.prev_live_sensors = live;
            }
        }
    }

    /// Recompute [`Activity::active_climb`] from the freshly-matched `progress_m`, applying
    /// enter/exit hysteresis over the raw [`Climbs::active_at`] lookup, and refill the resident
    /// [`climb_profile`](App::climb_profile) detail buffer **only on a new climb entry** (never per
    /// frame — the fill streams the climb's chunks).
    ///
    /// **Hysteresis.** The raw intervals carry no slack, so this widens them per the current state:
    /// while *off* a climb, a climb arms once progress reaches within [`CLIMB_ENTER_MARGIN_M`] of its
    /// base; while *on* a climb, it stays that climb until progress passes [`CLIMB_EXIT_MARGIN_M`]
    /// past its summit (or the rider has clearly moved onto a *different* climb's core interval).
    /// That asymmetric band is wider than the matcher's per-fix jitter, so straddling a boundary
    /// can't flap the on-climb state between consecutive fixes.
    ///
    /// **Off-route.** A stale match freezes `progress_m` (the matcher holds it while off-route), so
    /// leaving the route mid-climb *keeps* the current climb rather than snapping it away on a
    /// frozen cursor — the panel stays put until the rider rejoins and progress moves again. Only an
    /// explicit clear path (route swap/unload/replace) drops it.
    ///
    /// Called on each matched fix from [`tick`](App::tick) with the live route reader (the source
    /// the refill reads); a no-op that touches no SD when the active climb is unchanged.
    fn update_active_climb(&mut self, route: &RouteReader) {
        // Off-route freezes the cursor, so keep whatever climb we were on — don't recompute against
        // a stale progress. `apply_match` leaves `progress_m` frozen while off-route.
        if self.activity.off_route {
            return;
        }
        let prev = self.activity.active_climb;
        let next = resolve_active_climb(&self.climbs, self.activity.progress_m, prev);
        if next == prev {
            return; // unchanged — no refill, no SD read.
        }
        self.activity.active_climb = next;
        // Refill the single resident detail buffer for the new climb — only here, on the transition,
        // so a fix that stays on the same climb never re-reads the card.
        if let Some(seg) = next.and_then(|i| self.climbs.as_slice().get(i)) {
            self.climb_profile.fill(route, seg);
            #[cfg(test)]
            {
                self.climb_fill_count += 1;
            }
        }
        // The active climb changed: the riding views' climb-scoped readouts (and the Climb screen)
        // must repaint.
        self.map_dirty = true;
        // Host-driven auto-switch / auto-return (C5), off the same entry/exit edge.
        self.apply_climb_auto_switch(prev, next);
    }

    /// Recompute [`Activity::next_waypoint`] from the freshly-matched `progress_m` via the pure
    /// [`resolve_next_waypoint`], and slide a truncated table's window forward when the rider passes
    /// its tail — the waypoint twin of [`update_active_climb`](App::update_active_climb).
    ///
    /// **Off-route.** `apply_match` freezes `progress_m` off-route, so the index self-freezes; like
    /// the climb resolver, just don't fight that — return and hold whatever was next. (The chip is
    /// hidden off-route anyway; the along-route distance is meaningless there.)
    ///
    /// **Re-window on exhaustion.** A file with more than [`MAX_WAYPOINTS`](obc_route::MAX_WAYPOINTS)
    /// named waypoints loads only the first window and flags [`truncated`](obc_route::Waypoints).
    /// Once the rider has passed the resident tail (its linger included), reload from the current
    /// progress so the far waypoints keep tracking. Gated on `truncated`, so a normal route never
    /// re-streams; and the reload starts strictly past the old window (all its entries sit at
    /// `dist < progress`), so it can't re-fire on the next tick.
    ///
    /// Called on each matched fix from [`tick`](App::tick); touches SD only on the rare re-window.
    fn update_next_waypoint(&mut self, route: &RouteReader) {
        // Off-route freezes progress, so the resolved index freezes with it — keep what we had.
        if self.activity.off_route {
            return;
        }
        // Slide a truncated window forward once its whole resident span (last entry + linger) is
        // behind the rider — see the re-window note above.
        if self.waypoints.truncated {
            if let Some(last) = self.waypoints.as_slice().last() {
                if self.activity.progress_m >= last.dist_along_m.saturating_add(WAYPOINT_LINGER_M) {
                    self.waypoints = route.load_waypoints(self.activity.progress_m);
                    self.activity.next_waypoint = None; // the window slid — re-derive against it below
                }
            }
        }
        let prev = self.activity.next_waypoint;
        let next = resolve_next_waypoint(&self.waypoints, self.activity.progress_m, prev);
        if next != prev {
            self.activity.next_waypoint = next;
            self.map_dirty = true; // the next waypoint changed — the chip / fields must repaint
        }
    }

    /// The Auto-mode screen follow (epic #506, C5), driven off the climb entry/exit edge in
    /// [`update_active_climb`](App::update_active_climb) — the host-pushed-screen pattern (the P2
    /// precedent [`reconcile_upload_prompt`](App::reconcile_upload_prompt) uses), applied to the
    /// active-climb transition rather than a route upload:
    ///
    /// - **Entry** (`None → Some`): in [`Auto`](crate::settings::ClimbMode::Auto) mode, if the top
    ///   screen is a **riding view** (Map or Statistics — a [`ScreenKind::Riding`], never a menu /
    ///   overlay / ride-control / settings screen), switch it to the Climb screen. The riding-view
    ///   guard is the whole point: the rider deep in a menu or the pause page is never yanked out.
    /// - **Exit** (`Some → None`): if the top screen **is** the Climb screen — which it can only be
    ///   in Auto (an entry switch) or because the rider cycled there manually — return to the Map on
    ///   the crest, so a finished climb doesn't strand a stale "No climb" panel. This runs
    ///   regardless of mode: the Climb screen is only reachable with a climb active, so once the
    ///   climb ends there's nothing for it to show.
    ///
    /// [`Manual`](crate::settings::ClimbMode::Manual) and [`Off`](crate::settings::ClimbMode::Off)
    /// never *enter*; the exit return still fires from Manual (the rider cycled to the Climb screen
    /// themselves), but not from Off (the Climb screen is out of the ring, so the top is never it).
    /// A `Replace` (not a push) so the ring's depth is unchanged — the Climb screen is a sibling of
    /// the riding views, not an overlay.
    fn apply_climb_auto_switch(&mut self, prev: Option<usize>, next: Option<usize>) {
        use crate::screen::ScreenKind;
        let top_is = |app: &Self, want: fn(&Screen) -> bool| app.stack.last().is_some_and(want);
        match (prev, next) {
            // Entry: Auto + on a riding view → show the Climb screen.
            (None, Some(_))
                if self.settings.climb_mode == crate::settings::ClimbMode::Auto
                    && top_is(self, |s| s.kind() == ScreenKind::Riding) =>
            {
                if let Some(top) = self.stack.last_mut() {
                    *top = Screen::Climb(crate::screen::ClimbScreen::new());
                }
            }
            // Exit (crest): if we're sitting on the Climb screen, return to the Map.
            (Some(_), None) if top_is(self, |s| matches!(s, Screen::Climb(_))) => {
                if let Some(top) = self.stack.last_mut() {
                    *top = Screen::Map(MapScreen::new());
                }
            }
            _ => {}
        }
    }

    /// Whether the base screen shows live sensor data (user fix / ride accumulators) — Map,
    /// Statistics and Climb do, so a fresh fix must redraw them; Home and the menus don't. The base
    /// is the lowest *opaque* drawn screen, so an overlay (Ride control) over a riding view still
    /// counts as live since the map keeps moving under the pause panel.
    fn shows_live_data(&self) -> bool {
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        matches!(self.stack.get(base), Some(Screen::Map(_) | Screen::Statistics(_) | Screen::Climb(_)))
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
    /// Mirror the loaded map's routing-profile **names** into the App for the UI (routing-v2 N5,
    /// #538). The host calls this whenever it (re)loads a map's tables — pass
    /// [`Reader::nav_profiles`](obc_reader::Reader::nav_profiles) — exactly as it calls
    /// [`set_routes`](App::set_routes) when the route store changes. Copies only the display names
    /// (the multiplier tables stay in `MapTables`); the Bike-type settings screen cycles them and the
    /// created-route overview labels itself with the selected one. Safe to call on a router-less
    /// (`ble`) image — the names are map metadata and the setting still renders (inert). Dirties the
    /// map so an open settings screen picks up the new names.
    pub fn set_nav_profiles(&mut self, profiles: &[obc_reader::MapProfile]) {
        self.nav_profiles.set_from(profiles);
        self.map_dirty = true;
    }

    /// Feed the running firmware version string (T8 item 6) — the host calls this once at boot with
    /// its build's `git describe` tag (the same value the DFU confirm shows as "Installed"). The
    /// System settings screen's `Firmware` ledger row renders it (truncated to the 32-byte field,
    /// wrapped to a second line if it doesn't fit — never ellipsized).
    pub fn set_fw_version(&mut self, version: &str) {
        self.fw_version.clear();
        for ch in version.chars() {
            if self.fw_version.push(ch).is_err() {
                break;
            }
        }
    }

    /// Feed the loaded map's display name + OBCM format version (T8 item 6) — the host calls this on
    /// map load. The System screen's `Map` row reads it as `name · vN` (e.g. `grimsel · v10`).
    pub fn set_map_info(&mut self, name: &str, obcm_version: u8) {
        self.map_name.clear();
        for ch in name.chars() {
            if self.map_name.push(ch).is_err() {
                break;
            }
        }
        self.map_obcm_version = obcm_version;
    }

    /// Drain the one-shot **card-free scan request** (T8 item 6) the System settings screen posts on
    /// entry — the board answers a `true` with a FAT free-cluster scan → [`set_card_free`](App::set_card_free).
    pub fn take_card_scan_request(&mut self) -> bool {
        self.activity.take_card_scan_request()
    }

    /// Answer the card-free scan (T8 item 6): the host's free-space result in bytes, or `None` if the
    /// scan failed / is unavailable (the System screen keeps showing `--`). Dirties the frame so an
    /// open System screen repaints with the value.
    pub fn set_card_free(&mut self, bytes: Option<u64>) {
        self.card_free_bytes = bytes;
        self.map_dirty = true;
    }

    /// The loaded map's resident routing-profile names (read-only), for host inspection / tests.
    pub fn nav_profiles(&self) -> &crate::NavProfiles {
        &self.nav_profiles
    }

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
        // The climbs cache follows the same identity: it survives a rescan that keeps the route
        // (same-id remap), and drops when the navigated route vanishes. Clearing the active-climb
        // state too keeps a stale "on climb" flag from stranding the rider on a gone route.
        let old_climbs = self.climbs_route;
        self.climbs_route = old_climbs.and_then(remap);
        if old_climbs.is_some() && self.climbs_route.is_none() {
            self.climbs = Climbs::new();
            self.activity.active_climb = None;
        }
        // The waypoint table follows that same identity — remapped across a rescan, dropped (with the
        // next-waypoint index) when the navigated route vanishes.
        let old_wpts = self.waypoints_route;
        self.waypoints_route = old_wpts.and_then(remap);
        if old_wpts.is_some() && self.waypoints_route.is_none() {
            self.waypoints = Waypoints::new();
            self.activity.next_waypoint = None;
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
        // Re-point every held ride index by identity (its id in `old_ids` → new index), the
        // ride-namespace twin of the route remap: the Rides menu's highlight, an open Ride
        // detail's subject (#680 — a vanished subject becomes the detail's missing-ride state),
        // and the viewed-ride/profile keys the detail's band hangs off (identity survives → the
        // resident profile moves with it, no re-stream; vanished → the buffer drops below).
        let new_ids = &self.ride_catalog_ids;
        let remap = |i: usize| -> Option<usize> {
            let id = *old_ids.get(i)?;
            new_ids.iter().position(|&x| x == id)
        };
        let new_len = new_ids.len();
        for s in self.stack.iter_mut() {
            match s {
                Screen::Rides(m) => m.remap_rides(&remap, new_len),
                Screen::RideDetail(d) => d.remap_rides(&remap),
                _ => {}
            }
        }
        self.activity.viewed_ride = self.activity.viewed_ride.and_then(remap);
        self.ride_profile_for = self.ride_profile_for.and_then(remap);
        if self.ride_profile_for.is_none() {
            self.ride_profile = None; // the profiled ride vanished (or none was profiled)
        }
        self.ride_preview_for = self.ride_preview_for.and_then(remap);
        if self.ride_preview_for.is_none() {
            self.ride_preview.clear(); // the previewed ride vanished (or none was previewed)
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

    /// The Ride detail's pending **track request** (epic #678 T2 / #680): the durable object id of
    /// the ride whose recorded track the open detail screen wants profiled, or `None` when no
    /// detail is open / the resident buffer is already answered for it. The host's cue to stream
    /// `RD{id}.ORD` **once** (in chunks — never resident) and answer with
    /// [`set_ride_profile`](App::set_ride_profile) — obc-route's `ride_elevation_profile` is the
    /// shared builder. Resolved against the live [`ride_ids`](App::ride_ids), so a rescan racing
    /// the entry can't profile the wrong ride (a vanished index yields `None`, and the detail
    /// screen is showing its missing-ride state anyway). Re-polls until answered; answer `None`
    /// on a failed stream so a dead file isn't ground against every pass.
    pub fn take_ride_track_request(&mut self) -> Option<u16> {
        let viewed = self.activity.viewed_ride?;
        if self.ride_profile_for == Some(viewed) {
            return None; // already answered for this ride (profile or a recorded failure)
        }
        self.ride_catalog_ids.get(viewed).copied()
    }

    /// Park the host's answer to [`take_ride_track_request`](App::take_ride_track_request) in the
    /// app's single resident ride-profile buffer, keyed to the currently-viewed ride (`None` =
    /// the stream failed; the band keeps its loading note and the request doesn't re-fire).
    /// Dirties the map once — the open detail's band appears with the answer.
    pub fn set_ride_profile(&mut self, profile: Option<Profile>) {
        self.ride_profile = profile;
        self.ride_profile_for = self.activity.viewed_ride;
        self.map_dirty = true;
    }

    /// Hand in the viewed ride's decimated recorded-track shape polyline (#678 rework 3 — the
    /// Ride detail's track pager page): ≤ [`NAV_PREVIEW_MAX`] `(lon, lat)` µdeg points (more are
    /// truncated), built by obc-route's `ride_preview_polyline` in the **same host drain** as
    /// [`set_ride_profile`](App::set_ride_profile) (one `take_ride_track_request` answer fills
    /// both residents, so the file streams at most twice per entry, never per pass). Keyed to the
    /// currently-viewed ride; exit/rescan invalidation drops it alongside the profile.
    pub fn set_ride_preview(&mut self, pts: &[(i32, i32)]) {
        self.ride_preview.clear();
        for &p in pts.iter().take(NAV_PREVIEW_MAX) {
            let _ = self.ride_preview.push(p);
        }
        self.ride_preview_for = self.activity.viewed_ride;
        self.map_dirty = true;
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

    /// Post the **install-update request** (epic #615 S4/S5) — the one-shot the board's ride loop
    /// drains to run the DFU armer (validate `UPDATE.BIN`, snapshot the rollback, arm the
    /// boot-state page, reboot into the bootloader). The `dfu-install` debug-link command posts it
    /// **directly** (no confirm screen); the S5 UI reaches the same [`DfuAction::Install`] through
    /// the confirm screen instead. Posting records intent only — execution, guards (not
    /// mid-recording), and errors are the drain's.
    pub fn request_dfu_install(&mut self) {
        self.activity.request_dfu(crate::activity::DfuAction::Install);
    }

    /// Open the on-glass DFU check flow from a **remote** request — the BLE `installFw` command
    /// (epic #615 S6, #621): push the "Checking card..." wait and post
    /// [`DfuAction::Scan`](crate::activity::DfuAction), exactly the System menu's press arriving
    /// over the air. **Never `Install`** — a remote request can only open the scan → confirm flow;
    /// the encoder press on the confirm screen is what posts the arm (spec §4.4: the phone can
    /// request, only the rider installs; the direct-Install path stays the physical debug link's).
    ///
    /// Returns `true` when the flow opened (the board consumes its pending request); `false`
    /// **defers** — the board keeps the request pending and retries next pass, so an inconvenient
    /// moment delays the card, never drops or force-installs it. Deferred while:
    /// - the passkey card is up or a hold is charging (the
    ///   [`reconcile_update_toast`](App::reconcile_update_toast) politeness — never cover the
    ///   pairing code, never land mid-hold),
    /// - a DFU screen (check / confirm / progress / error) is already on the stack — never
    ///   double-open, and never yank a flow the rider opened from the menu themself,
    /// - a [`DfuAction`] is already posted but undrained (don't overwrite a phase in flight),
    /// - a ride is recording (defensive: the BLE edge already answered `busy`, but recording can
    ///   start between that reply and this drain).
    pub fn open_remote_dfu_check(&mut self) -> bool {
        let dfu_screen_up = self.stack.iter().any(|s| {
            matches!(s, Screen::DfuCheck(_) | Screen::DfuConfirm(_) | Screen::DfuProgress(_) | Screen::DfuError(_))
        });
        if self.passkey_card_up()
            || self.hold_charging()
            || dfu_screen_up
            || self.activity.has_dfu_request()
            || self.activity.is_tracking()
        {
            return false;
        }
        self.activity.request_dfu(crate::activity::DfuAction::Scan);
        let r = self.stack.push(Screen::DfuCheck(crate::screen::DfuCheckScreen::new()));
        debug_assert!(r.is_ok(), "screen stack overflow — raise MAX_DEPTH");
        self.map_dirty = true;
        true
    }

    /// Drain the pending [`DfuAction`](crate::activity::DfuAction) — the board's per-pass one-shot,
    /// like [`take_nav_request`](App::take_nav_request). `Some(Scan)` runs the read-only validation
    /// and answers via [`notify_dfu_scan_result`](App::notify_dfu_scan_result); `Some(Install)`
    /// arms and reboots.
    pub fn take_dfu_request(&mut self) -> Option<crate::activity::DfuAction> {
        self.activity.take_dfu_request()
    }

    /// The board's answer to a drained [`DfuAction::Scan`](crate::activity::DfuAction) (epic #615
    /// S5, #620) — the "Checking card..." wait's result. Mirrors
    /// [`notify_nav_result`](App::notify_nav_result): the answer lands in the
    /// [`DfuCheck`](crate::screen::DfuCheckScreen) screen the System menu pushed, **replacing** it
    /// with the confirm screen (`Ok`) or the error card (`Err`). If that screen is gone — the rider
    /// pressed Back out of the wait — the answer is dropped (nothing was armed; a scan costs
    /// nothing).
    pub fn notify_dfu_scan_result(&mut self, result: Result<crate::dfu::DfuScanReport, crate::dfu::DfuScanError>) {
        let Some(i) = self.stack.iter().position(|s| matches!(s, Screen::DfuCheck(_))) else {
            return;
        };
        self.stack[i] = match result {
            Ok(report) => Screen::DfuConfirm(crate::screen::DfuConfirmScreen::new(report)),
            Err(e) => Screen::DfuError(crate::screen::DfuErrorScreen::new(e)),
        };
        self.map_dirty = true;
    }

    /// Record that this boot **confirmed a freshly-installed firmware update** (S4, #619): the
    /// board calls this right after the trial confirm writes `Idle { installed }` at the health
    /// anchor (first frame presented + SD mounted). `version` is the running image's OBCU
    /// version string (≤ 32 bytes by the container format; longer input is truncated).
    pub fn notify_update_confirmed(&mut self, version: &str) {
        let mut v: heapless::String<32> = heapless::String::new();
        let mut end = version.len().min(v.capacity());
        while end > 0 && !version.is_char_boundary(end) {
            end -= 1;
        }
        let _ = v.push_str(&version[..end]);
        self.update_confirmed = Some(v);
    }

    /// Take the one-time "update confirmed" fact — the just-confirmed running version, if this
    /// boot set one. S5's toast is the consumer; taking it clears it (shown once).
    pub fn take_update_confirmed(&mut self) -> Option<heapless::String<32>> {
        self.update_confirmed.take()
    }

    /// Record that this boot **detected a failed firmware update** — the board's boot-outcome
    /// reconcile found the armed update is not the running firmware (`why` says how far it got);
    /// `staged` is the failed image's version string when the arm marker survived. The one-time
    /// fact the "UPDATE FAILED" card takes, delivered by the same reconcile pass as the success
    /// toast.
    pub fn notify_update_failed(&mut self, why: crate::dfu::DfuFailure, staged: Option<&str>) {
        self.update_failed = Some((why, staged.map(crate::dfu::clamp)));
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

    /// **Debug / snapshot only** (epic #506, C4): open the [`Climb`](crate::screen::ClimbScreen)
    /// screen directly. The screen isn't reachable through any gesture until C5 wires its Back-cycle
    /// and auto-switch, so the UI-snapshot sweep drives it through this seam (the sim's `--open-climb`
    /// flag) to capture the striped-profile PNG. Replaces the current base riding view (Map) rather
    /// than stacking over it, so the frame is exactly the Climb screen; a no-op if a climb isn't
    /// active (nothing to draw). No production path reaches this.
    pub fn debug_open_climb(&mut self) {
        if self.activity.active_climb.is_none() {
            return;
        }
        if let Some(top) = self.stack.last_mut() {
            *top = Screen::Climb(crate::screen::ClimbScreen::new());
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
    /// [`Exhausted`](obc_route::nav::NavError::Exhausted) → "Too far to route here." (there is no
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
                self.climbs = Climbs::new();
                self.climbs_route = None; // re-segmented from the fresh geometry on the next tick
                self.activity.active_climb = None;
                self.waypoints = Waypoints::new();
                self.waypoints_route = None; // re-loaded from the fresh geometry on the next tick
                self.activity.next_waypoint = None;
                self.activity.progress_m = 0;
                self.activity.off_route = false;
                self.activity.dist_to_route_m = 0;
                // Activate for the preview (the overview contract: the host streams the geometry
                // while the page shows); `prev_active` restores whatever was loaded on cancel.
                let prev = self.activity.active_route;
                self.activity.active_route = Some(idx);
                // Every plan starts preview-less (#685 §4): a re-route commits new bytes under
                // the same id/index, so an old shape must never survive into the new overview.
                // The host hands the fresh decimated polyline via `set_nav_preview` (the sim's
                // commit tail does it in the same pass; the board on the next one).
                self.nav_preview.clear();
                self.nav_preview_route = None;
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

    /// Whether a Route overview is up **without** its route-shape preview (#685 §4; #678 rework 3
    /// widened it from the computed overview to every overview — the stored-route page's track
    /// pager wants the shape too) — the host's per-pass cue to decimate the active route's
    /// polyline ([`RouteReader::preview_polyline`](obc_route::RouteReader::preview_polyline)) and
    /// hand it to [`set_nav_preview`](App::set_nav_preview). Entering the overview points
    /// [`active_route`](Activity::active_route) at the previewed route (the same key the
    /// elevation-profile rebuild streams on), so the fill runs once per overview entry — `false`
    /// the moment the preview is in (or the overview is gone), never per pass.
    pub fn nav_preview_missing(&self) -> bool {
        self.stack.iter().any(|s| matches!(s, Screen::RouteOverview(_)))
            && self.nav_preview_route != self.activity.active_route
    }

    /// Hand in the previewed route's decimated shape polyline (#685 §4) — ≤
    /// [`NAV_PREVIEW_MAX`] `(lon, lat)` µdeg points (more are truncated), **decimated host-side**
    /// (the sim/web hosts' per-pass fill; the board's ride loop; a plan's commit tail). Keyed to
    /// the current [`active_route`](Activity::active_route) — the route the overview activated —
    /// so a later route change stales it automatically.
    pub fn set_nav_preview(&mut self, pts: &[(i32, i32)]) {
        self.nav_preview.clear();
        for &p in pts.iter().take(NAV_PREVIEW_MAX) {
            let _ = self.nav_preview.push(p);
        }
        self.nav_preview_route = self.activity.active_route;
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
    /// re-renders the map on the Map / Statistics / Climb screens, which deliberately omit the glyph.
    fn indicator_visible(&self) -> bool {
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        !matches!(self.stack.get(base), Some(Screen::Map(_) | Screen::Statistics(_) | Screen::Climb(_)))
    }

    /// The live BLE pairing passkey, or `None` when not pairing — [`BleStatus::passkey`](crate::BleStatus)
    /// as last fed to [`set_ble_status`](App::set_ble_status). Consumed by the passkey card in P2
    /// (#449); exposed now so the seam is observable end to end.
    pub fn ble_passkey(&self) -> Option<u32> {
        self.ble_passkey
    }

    // ==================== BLE sensor seam (epic #707, SE7) ====================

    /// Feed the host's per-slot **sensor status** ([`SensorStatus`](crate::sensors::SensorStatus)) —
    /// the central manager's HR / power / cadence connection phase + battery + live tick, distilled to
    /// app vocabulary and pushed each pass (the board's `ble::sensors` snapshot, or the sim's fake
    /// manager). Stored app-side like [`set_ble_status`](App::set_ble_status); no radio type crosses
    /// the seam. Up to [`SENSOR_SLOTS`](crate::settings::SENSOR_SLOTS) slots are copied (extra ignored).
    ///
    /// A change **while the Sensors screen is up** dirties the map so the status lines repaint; on any
    /// other screen the status isn't drawn, so an update — fed every pass — repaints nothing.
    pub fn set_sensor_status(&mut self, status: &[crate::sensors::SensorStatus]) {
        let mut next = self.sensor_status;
        for (dst, src) in next.iter_mut().zip(status) {
            *dst = *src;
        }
        if next != self.sensor_status {
            self.sensor_status = next;
            if self.sensors_screen_up() {
                self.map_dirty = true;
            }
        }
    }

    /// Feed the host's live **sensor scan hits** ([`SensorScanHit`](crate::sensors::SensorScanHit)) —
    /// the sensors discovered while the scan-list screen runs a scan. Replaces the resident list
    /// wholesale (up to [`SCAN_HITS_MAX`](crate::sensors::SCAN_HITS_MAX)); an empty slice clears it
    /// (the host feeds `&[]` when no scan is active). A change while the scan screen is up dirties the
    /// map so a freshly-found sensor appears without waiting for another input.
    pub fn set_sensor_scan_hits(&mut self, hits: &[crate::sensors::SensorScanHit]) {
        let changed =
            self.sensor_scan_hits.len() != hits.len() || self.sensor_scan_hits.iter().zip(hits).any(|(a, b)| a != b);
        if !changed {
            return;
        }
        self.sensor_scan_hits.clear();
        for h in hits.iter().take(crate::sensors::SCAN_HITS_MAX) {
            let _ = self.sensor_scan_hits.push(h.clone());
        }
        if self.sensors_screen_up() {
            self.map_dirty = true;
        }
    }

    /// The per-slot sensor status as last fed to [`set_sensor_status`](App::set_sensor_status) — the
    /// Sensors screen's row source, and how a test observes the seam end to end.
    pub fn sensor_status(&self) -> &[crate::sensors::SensorStatus] {
        &self.sensor_status
    }

    /// The live sensor scan hits as last fed to [`set_sensor_scan_hits`](App::set_sensor_scan_hits) —
    /// the scan-list screen's rows.
    pub fn sensor_scan_hits(&self) -> &[crate::sensors::SensorScanHit] {
        &self.sensor_scan_hits
    }

    /// Whether the rider is on the **scan-list** screen and a scan should run (SE7) — the level the
    /// Sensors screen raises on entry to a row and lowers on exit/Back
    /// ([`Activity::request_sensor_scan`](crate::activity::Activity)). The host reads it each pass (the
    /// `set_radio_enabled` shape): while `true` it keeps a discovery scan running and feeds the hits
    /// back; when it falls it clears the app scan list.
    pub fn sensor_scan_active(&self) -> bool {
        self.activity.sensor_scan_active()
    }

    /// Whether the Sensors settings screen (its row list or a scan list) is the top screen — gates the
    /// sensor-seam repaint so a status/scan-hit update dirties the map only where it's drawn.
    fn sensors_screen_up(&self) -> bool {
        matches!(self.stack.last(), Some(Screen::Sensors(_) | Screen::SensorScan(_)))
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
    ///
    /// `elevation` is the route's mini sparkline ([`obc_route::elevation_sparkline`], `None` when
    /// the route has no elevation), which the host builds from the just-committed OBCR at commit
    /// time and the idle "ROUTE RECEIVED" card draws (#682). Carried with the event so a
    /// hold-deferred delivery still has it; the swap / active-replace variants ignore it.
    pub fn notify_route_uploaded(
        &mut self,
        id: u16,
        replaced: bool,
        elevation: Option<[u8; obc_route::SPARKLINE_BUCKETS]>,
    ) {
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
            self.climbs = Climbs::new();
            self.climbs_route = None; // the next tick re-segments from the reopened geometry
            self.activity.active_climb = None;
            self.waypoints = Waypoints::new();
            self.waypoints_route = None; // the next tick re-loads from the reopened geometry
            self.activity.next_waypoint = None;
            self.activity.progress_m = 0;
            self.activity.off_route = false;
            self.activity.dist_to_route_m = 0;
            self.map_dirty = true; // the drawn route line + progress changed under the rider
        }
        self.pending_upload = Some(UploadEvent { id, active_replace, elevation });
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
            Screen::RouteReceived(crate::screen::RouteReceivedScreen::new(idx, self.now_ms, ev.elevation))
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

    /// Surface the one-time post-update verdict — the "Updated to vX" toast (epic #615 S5, #620)
    /// if this boot confirmed a freshly-installed update, or its failure twin, the "UPDATE FAILED"
    /// card, if the boot-outcome reconcile found the armed update is not what's running. The board
    /// calls [`notify_update_confirmed`](App::notify_update_confirmed) at the health anchor (the
    /// first frame with the SD mounted) or [`notify_update_failed`](App::notify_update_failed) at
    /// boot; the next [`advance_animations`](App::advance_animations) pass drains the fact and
    /// pushes the card once. Deferred behind a
    /// passkey card or a live hold like [`reconcile_warning`](App::reconcile_warning), so it never
    /// covers the pairing code or lands mid-hold; a normal boot has no fact and does nothing.
    fn reconcile_update_toast(&mut self) {
        if self.update_confirmed.is_none() && self.update_failed.is_none() {
            return;
        }
        if self.passkey_card_up() || self.hold_charging() {
            return; // retried next pass, once the card clears / the hold resolves
        }
        if let Some(version) = self.update_confirmed.take() {
            let r = self.stack.push(Screen::DfuUpdated(crate::screen::DfuUpdatedScreen::new(&version)));
            debug_assert!(r.is_ok(), "screen stack overflow — raise MAX_DEPTH");
            self.map_dirty = true;
        }
        // The failure twin (the board's boot-outcome reconcile sets at most one of the two facts).
        if let Some((why, staged)) = self.update_failed.take() {
            let card = crate::screen::DfuFailedScreen::new(why, staged.as_deref());
            let r = self.stack.push(Screen::DfuFailed(card));
            debug_assert!(r.is_ok(), "screen stack overflow — raise MAX_DEPTH");
            self.map_dirty = true;
        }
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

    /// Whether the wall clock has an **established** set-point — a persisted/manual/GPS time has been
    /// applied, versus a fresh clock that has never been told the time (see
    /// [`WallClock::is_established`](crate::wall_clock::WallClock::is_established)). The Home date
    /// line gates on this so it never shows a date with no trusted origin.
    pub fn clock_is_set(&self) -> bool {
        self.wall_clock.is_established()
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
            // The per-ride BLE-sensor summary heads the v2 ride object (epic #707, SE3). Each is
            // `None` (→ sentinel) when the ride saw no fresh sample of that quantity.
            avg_hr: self.activity.avg_hr(),
            max_hr: self.activity.max_hr(),
            avg_cadence: self.activity.avg_cadence(),
            avg_power: self.activity.avg_power(),
            max_power: self.activity.max_power(),
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
        // Any recognised gesture is user activity: reset the idle-return clock (see
        // `apply_idle_return`). A gesture the screen ignores still counts — a turn on Home, say.
        self.last_input_ms = self.now_ms;
        // Snapshot the settings so a settings-screen edit is detected by one `==` (Settings is
        // `Copy + Eq`). A change flags a save for the host to pick up via `take_settings_dirty`.
        let settings_before = self.settings;
        let App {
            state,
            activity,
            settings,
            catalog,
            ride_catalog,
            nav_profiles,
            stack,
            now_ms,
            poi_scratch,
            sensor_scan_hits,
            ..
        } = self;
        let mut cx = Ctx {
            state,
            activity,
            settings,
            routes: catalog.as_slice(),
            rides: ride_catalog.as_slice(),
            nav_profiles,
            poi_scratch,
            sensor_scan_hits: sensor_scan_hits.as_slice(),
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
        let pan_active = self.state.pan.is_some();
        let tracking = self.activity.is_tracking();
        for scr in self.stack.iter_mut().skip(base) {
            let tick = scr.tick_timers(self.now_ms, now, ms_to_next_minute, &self.settings, w, h, pan_active, tracking);
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
        // Before the idle sweep, so a warning that lands this pass is on top when the sweep checks
        // its exemptions — an unacknowledged card must not be yanked to Home by the idle return.
        self.reconcile_warning();
        // The one-time post-update toast (epic #615 S5): land it after the warning reconcile so it
        // sits on top when the idle sweep checks its exemptions (an unacknowledged card must not be
        // yanked Home). A normal boot has no confirmed-update fact and this is a cheap no-op.
        self.reconcile_update_toast();
        // The idle-return sweep (fire the return if we're past the deadline) and its residual wake,
        // folded into the deadline the event-driven host arms so a parked device wakes to return.
        self.apply_idle_return();
        if let Some(rem) = self.idle_return_remaining_ms() {
            self.next_wake_ms = Some(self.next_wake_ms.map_or(rem, |w| w.min(rem)));
        }
    }

    /// Millis until the idle-return timeout expires, or `None` when no return is pending — the
    /// mechanism is off ([`Never`](crate::settings::IdleReturn::Never)), a modal exemption is up, or
    /// we're already at the target screen (Home when idle, a ride view while tracking), so no idle
    /// wake is owed. At least `1` while pending, so a due return has already fired this pass and the
    /// wake is strictly future.
    fn idle_return_remaining_ms(&self) -> Option<u32> {
        let timeout = self.settings.idle_return.timeout_ms()?;
        if !self.idle_return_pending() {
            return None;
        }
        let elapsed = self.now_ms.wrapping_sub(self.last_input_ms);
        Some(timeout.saturating_sub(elapsed).max(1))
    }

    /// Whether an idle return would actually *move* somewhere — false when a modal exemption is up,
    /// or we're already where the timeout would land (the Home root when not tracking, a deliberate
    /// ride view while tracking). Gates both the idle wake and the sweep so an already-arrived
    /// device arms no needless wake and re-checks nothing each tick.
    fn idle_return_pending(&self) -> bool {
        if self.idle_return_exempt() {
            return false;
        }
        if self.activity.is_tracking() {
            !self.is_ride_view()
        } else {
            // Not tracking: any overlay above the Home root would return to Home — **except** the
            // route-less browse Map (Menu → Map). Riding with the map open without recording is a
            // deliberate view, not idleness, so it's exempt just like a ride view is mid-ride.
            self.stack.len() > 1 && !matches!(self.stack.last(), Some(Screen::Map(_)))
        }
    }

    /// Whether the current top screen is **exempt** from the idle-return timeout — the modal cards
    /// that must stay put until dismissed (the BLE passkey card, the three route-received /
    /// -updated / -swap popups, the #504 sensor/storage warning card) and the route-planning
    /// spinner (a multi-second wait that isn't idleness). While one of these is up, no idle return
    /// fires and no idle wake is armed.
    fn idle_return_exempt(&self) -> bool {
        matches!(
            self.stack.last(),
            Some(
                Screen::Passkey(_)
                    | Screen::RouteReceived(_)
                    | Screen::RouteUpdated(_)
                    | Screen::RouteSwap(_)
                    | Screen::NavPlanning(_)
                    | Screen::Warning(_)
                    // The whole SD-sideload update flow (epic #615 S5): a card/wait the rider is
                    // acting on — never yank it Home mid-flow (the progress screen ends in a reboot).
                    | Screen::DfuCheck(_)
                    | Screen::DfuConfirm(_)
                    | Screen::DfuProgress(_)
                    | Screen::DfuError(_)
                    | Screen::DfuUpdated(_)
                    | Screen::DfuFailed(_)
            )
        )
    }

    /// Whether the current top screen is one of the **deliberate ride views** that must never time
    /// out while a ride is being tracked — the Map (the ride base), Statistics, Climb, and the
    /// Paused / Ride-control page. A rider sitting on any of these is watching live ride data, not
    /// lost in a menu. Every *other* screen (menus, lists, settings, route overview) returns to the
    /// Map on the idle timeout when tracking.
    fn is_ride_view(&self) -> bool {
        matches!(
            self.stack.last(),
            Some(Screen::Map(_) | Screen::Statistics(_) | Screen::Climb(_) | Screen::RideControl(_))
        )
    }

    /// Navigate "back to where it belongs" once the idle-return timeout ([`idle_return`]) has
    /// elapsed with no user input — the app-level counterpart to the popups' timeout-dismiss sweep,
    /// run once per [`advance_animations`](App::advance_animations) pass.
    ///
    /// - **Not tracking a ride:** from any screen *except* the route-less browse Map (Menu → Map, a
    ///   deliberate view — see [`idle_return_pending`](App::idle_return_pending)), clear every
    ///   overlay back to the Home root and reseed the screensaver backdrop (as a manual return does).
    /// - **Tracking a ride:** a menu / list / settings / overview screen returns to the Map (the
    ///   ride base). The deliberate ride views ([`is_ride_view`](App::is_ride_view)) stay put.
    ///
    /// Never fires while the timeout is disabled ([`Never`]), a modal exemption is up
    /// ([`idle_return_exempt`](App::idle_return_exempt)), a hold is charging (a gesture in progress
    /// is activity — deferred a tick, like the popup sweeps), or we're already at the target screen.
    ///
    /// [`idle_return`]: crate::settings::Settings::idle_return
    /// [`Never`]: crate::settings::IdleReturn::Never
    fn apply_idle_return(&mut self) {
        let Some(timeout) = self.settings.idle_return.timeout_ms() else { return };
        // Nothing to move (already home / on a ride view), a modal exemption is up, or a hold is
        // charging (a gesture in progress is activity): defer, exactly like the popup sweeps.
        if !self.idle_return_pending() || self.hold_charging() {
            return;
        }
        if self.now_ms.wrapping_sub(self.last_input_ms) < timeout {
            return;
        }
        // Past the deadline: consume it so the return fires once, not every pass hereafter.
        self.last_input_ms = self.now_ms;
        self.map_dirty = true;
        if self.activity.is_tracking() {
            // Mid-ride, on a non-ride screen: return to the Map (the ride base).
            self.stack.truncate(1); // drop back toward the root…
            let _ = self.stack.push(Screen::Map(MapScreen::new())); // …then land on the Map
        } else {
            // Not tracking: clear to the Home root and reseed the screensaver (as a manual return does).
            self.stack.truncate(1);
            if let Some(Screen::Home(home)) = self.stack.first_mut() {
                home.reseed(self.now_ms);
            }
        }
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
        // Invalidate the resident **ride** profile + track preview the moment they stop matching
        // the viewed ride (#680; the preview joined in #678 rework 3): the detail exited
        // (`viewed_ride` cleared) or moved subjects. Filling is the host's (`set_ride_profile` /
        // `set_ride_preview`); only the drop lives here, so a stale band/shape is never drawn.
        if self.ride_profile_for != self.activity.viewed_ride {
            self.ride_profile = None;
            self.ride_profile_for = None;
        }
        if self.ride_preview_for != self.activity.viewed_ride {
            self.ride_preview.clear();
            self.ride_preview_for = None;
        }

        // Computed before the field borrow below splits `self`.
        let now = self.wall_clock.now(self.now_ms);
        let clock_set = self.wall_clock.is_established();
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
            nav_profiles,
            renderer,
            stack,
            now_ms,
            profile,
            ride_profile,
            climbs,
            climb_profile,
            waypoints,
            breadcrumb,
            poi_scratch,
            fw_version,
            map_name,
            map_obcm_version,
            card_free_bytes,
            nav_preview,
            nav_preview_route,
            ride_preview,
            ride_preview_for,
            sensor_status,
            sensor_scan_hits,
            ..
        } = self;
        // The shape previews draw only for the subject they were decimated for — a stale key
        // (route/ride changed, preview not re-fed yet) hands the screens an empty slice.
        let nav_preview: &[(i32, i32)] =
            if nav_preview_route.is_some() && *nav_preview_route == activity.active_route { nav_preview } else { &[] };
        let ride_preview: &[(i32, i32)] =
            if ride_preview_for.is_some() && *ride_preview_for == activity.viewed_ride { ride_preview } else { &[] };
        // Bundle the active climb for the screens: the resident detail buffer is only meaningful
        // when a climb is active, so hand out the `(seg, profile)` pair exactly when `active_climb`
        // resolves to a live segment — a stale buffer is never reachable through `Render`.
        let climb = activity
            .active_climb
            .and_then(|i| climbs.as_slice().get(i))
            .map(|seg| screen::ActiveClimb { seg, profile: &*climb_profile });
        let mut rx = Render {
            reader,
            renderer,
            state,
            activity,
            settings,
            routes: catalog.as_slice(),
            rides: ride_catalog.as_slice(),
            nav_profiles,
            route,
            profile: profile.as_ref(),
            ride_profile: ride_profile.as_ref(),
            climb,
            waypoints: &*waypoints,
            breadcrumb: &*breadcrumb,
            nav_preview,
            ride_preview,
            poi_scratch,
            sensor_status: sensor_status.as_slice(),
            sensor_scan_hits: sensor_scan_hits.as_slice(),
            w: w as i32,
            h: h as i32,
            now_ms: *now_ms,
            now,
            clock_set,
            hold_progress,
            no_fix,
            clock,
            stats: RenderStats::default(),
            fw_version: fw_version.as_str(),
            map_name: map_name.as_str(),
            map_obcm_version: *map_obcm_version,
            card_free_bytes: *card_free_bytes,
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
                hr: None,
                power: None,
                cadence: None,
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
                hr: None,
                power: None,
                cadence: None,
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
                hr: None,
                power: None,
                cadence: None,
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
        fn record(&mut self, _p: obc_route::TrackPoint) -> Result<(), crate::hal::TrackError> {
            self.0 += 1;
            Ok(())
        }
    }

    /// A track sink whose every append fails — the "card pulled / write error mid-ride" case.
    struct FailSink;
    impl crate::hal::TrackSink for FailSink {
        fn record(&mut self, _p: obc_route::TrackPoint) -> Result<(), crate::hal::TrackError> {
            Err(crate::hal::TrackError)
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
                hr: None,
                power: None,
                cadence: None,
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
                hr: None,
                power: None,
                cadence: None,
            },
            None,
        );
        assert!(app.has_live_fix(2_000), "the fix landed → banner clears");
        assert_eq!(sink.0, 1, "the first fix logs the segment anchor");
    }

    /// A failed ride-log append (card pulled / write error mid-ride) must not be swallowed: the app
    /// raises the dismissable "recording error" warning so the rider learns the log dropped a point
    /// — the core of issue #11. Latched once per boot: a whole ride of failing writes is one card.
    #[test]
    fn record_failure_raises_recording_error_warning() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.activity.start_session();

        // No warning while nothing has failed.
        assert!(!app.stack.iter().any(|s| matches!(s, Screen::Warning(_))), "a healthy ride shows no warning card",);

        // A logged fix whose write fails → the recording-error card opens.
        let mut sink = FailSink;
        let mut loc = OneFix(Some(Fix::at(0, 0)));
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
                hr: None,
                power: None,
                cadence: None,
            },
            None,
        );
        let card = app
            .stack
            .iter()
            .find_map(|s| match s {
                Screen::Warning(w) => Some(w.flags()),
                _ => None,
            })
            .expect("a failed record opens the recording-error card");
        assert!(card.contains(WarningFlags::REC_ERROR), "the card carries the recording-error flag");

        // Dismiss it; a second failing fix doesn't nag again (latched once per boot). A small,
        // plausible move (~11 m in 1 s) so the fix is actually logged — record is called and fails
        // again, which the latch must swallow.
        app.apply_gesture(Gesture::Back);
        assert!(!app.stack.iter().any(|s| matches!(s, Screen::Warning(_))), "dismiss pops the card");
        let mut loc = OneFix(Some(Fix::at(0, 100)));
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
                hr: None,
                power: None,
                cadence: None,
            },
            None,
        );
        assert!(
            !app.stack.iter().any(|s| matches!(s, Screen::Warning(_))),
            "an already-acknowledged recording error stays quiet",
        );
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
                hr: None,
                power: None,
                cadence: None,
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
        assert!(placed.climbs.is_empty() && placed.climbs_route.is_none(), "no climbs before a route loads");
        assert!(placed.activity.active_climb.is_none(), "not on a climb at power-on");
        assert!(placed.waypoints.is_empty() && placed.waypoints_route.is_none(), "no waypoints before a route loads");
        assert!(placed.activity.next_waypoint.is_none(), "no next waypoint at power-on");
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
                hr: None,
                power: None,
                cadence: None,
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

    // --- end-to-end BLE sensor seam through `tick` (SE2, #709) ---

    /// A heart-rate strap that yields one sample then runs dry (the fresh-mailbox contract).
    struct OneHr(Option<u16>);
    impl crate::hal::HeartRateSource for OneHr {
        fn poll(&mut self) -> Option<u16> {
            self.0.take()
        }
    }

    /// A power meter that yields one sample then runs dry.
    struct OnePower(Option<u16>);
    impl crate::hal::PowerSource for OnePower {
        fn poll(&mut self) -> Option<u16> {
            self.0.take()
        }
    }

    /// A cadence sensor that yields one sample then runs dry.
    struct OneCadence(Option<u8>);
    impl crate::hal::CadenceSource for OneCadence {
        fn poll(&mut self) -> Option<u8> {
            self.0.take()
        }
    }

    /// The `tick` → `poll` → `record_*` → `accumulate` wiring for all three BLE sensor drains. The
    /// samples arrive **only on the tick that closes the moving interval**: because the drains run
    /// *before* `record_motion`, that same tick's interval must book them — if the drain order ever
    /// regressed to after the fix, the summary accessors would read `None` here.
    #[test]
    fn tick_drains_ble_sensors_into_live_values_and_summaries() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // boots Riding
        const STEP_UD: i32 = 45; // ~5 m of latitude — one second at ~5 m/s, comfortably moving

        // t = 1 s: the motion anchor, no sensor samples yet — everything reads `--`.
        tick_fix(&mut app, Fix::at(0, 0), 1_000);
        assert_eq!(app.activity.live_hr(1_000), None, "no sample yet → no live HR");
        assert_eq!(app.activity.avg_hr(), None);

        // t = 2 s: a moving fix *and* one fresh sample per sensor, all through `Sensors`.
        app.now_ms = 2_000;
        let mut loc = OneFix(Some(Fix::at(STEP_UD, 0)));
        let mut hr = OneHr(Some(150));
        let mut power = OnePower(Some(250));
        let mut cadence = OneCadence(Some(90));
        app.tick(
            RideClock(2_000),
            Sensors {
                loc: &mut loc,
                altimeter: None,
                temperature: None,
                clock: None,
                compass: None,
                track: None,
                fuel: None,
                hr: Some(&mut hr),
                power: Some(&mut power),
                cadence: Some(&mut cadence),
            },
            None,
        );

        // Live: each poll landed in Activity, timestamped at this tick.
        assert_eq!(app.activity.live_hr(2_000), Some(150), "tick drained the HR strap");
        assert_eq!(app.activity.live_power(2_000), Some(250), "tick drained the power meter");
        assert_eq!(app.activity.live_cadence(2_000), Some(90), "tick drained the cadence sensor");
        // Summaries: the same-tick samples were booked into the same tick's moving interval —
        // proving the drains run before `record_motion` (else `hr_ms` would still be 0 → `None`).
        assert_eq!(app.activity.avg_hr(), Some(150), "the moving interval booked the fresh HR");
        assert_eq!(app.activity.max_hr(), Some(150));
        assert_eq!(app.activity.avg_power(), Some(250), "…and the fresh power");
        assert_eq!(app.activity.max_power(), Some(250));
        assert_eq!(app.activity.avg_cadence(), Some(90), "…and the fresh cadence");
    }

    /// The stat tiles judge sensor freshness with the `live_*_display` accessors, which compare
    /// against the last `tick`'s `RideClock` (`Activity::note_sensor_clock`) — the clock the samples
    /// record on — **not** the render-time `self.now_ms`. On the board those are one monotonic `now`;
    /// in the simulator mid GPX replay they diverge (record on playback time, render on wall time),
    /// and a tile keyed on the render clock blanked to `--` within `SENSOR_STALE_MS` — Timo's "the
    /// values showed up once, then only dashes." This pins the fix: `_display` stays fresh across the
    /// divergence, while the raw render-clock read is what used to (wrongly) blank.
    #[test]
    fn sensor_tile_display_survives_render_clock_divergence() {
        // The old sim mid-replay: sample recorded on playback time (30 s), but the render/map-plane
        // clock ran on wall time (90 s) — a 60 s gap > SENSOR_STALE_MS.
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.now_ms = 90_000; // wall clock, far ahead of the replay's playback clock
        let mut loc = OneFix(None);
        let mut hr = OneHr(Some(142));
        app.tick(
            RideClock(30_000), // playback time — the clock the HR sample records on
            Sensors {
                loc: &mut loc,
                altimeter: None,
                temperature: None,
                clock: None,
                compass: None,
                track: None,
                fuel: None,
                hr: Some(&mut hr),
                power: None,
                cadence: None,
            },
            None,
        );
        // The tile path: fresh, because it compares against the recorded-on clock (30 s), not 90 s.
        assert_eq!(app.activity.live_hr_display(), Some(142), "the tile shows the value across the divergence");
        // The old, wrong path — reading against the render clock — is what blanked the tile.
        assert_eq!(
            app.activity.live_hr(app.now_ms),
            None,
            "the render-clock read is stale (90 s vs a 30 s sample) — the bug `_display` fixes"
        );

        // And staleness still works on the ride clock: advance the tick clock 6 s past the sample
        // with no new reading → the tile blanks, exactly as a dropped strap should.
        app.activity.note_sensor_clock(36_001);
        assert_eq!(app.activity.live_hr_display(), None, "a >5 s-old sample still blanks — no frozen value");
    }

    /// One tick with only an HR sample (no fix, nothing else moving): `loc` yields `None` so the
    /// `AppState` comparison is a no-op — any repaint demand is the sensor-tile edge alone.
    fn tick_hr_only(app: &mut App, bpm: Option<u16>, at_ms: u32) {
        app.now_ms = at_ms;
        let mut loc = OneFix(None);
        let mut hr = OneHr(bpm);
        app.tick(
            RideClock(at_ms),
            Sensors {
                loc: &mut loc,
                altimeter: None,
                temperature: None,
                clock: None,
                compass: None,
                track: None,
                fuel: None,
                hr: Some(&mut hr),
                power: None,
                cadence: None,
            },
            None,
        );
    }

    /// Epic #744 SR3: a fresh BLE sample lands in `Activity`, which the `state != state_before`
    /// redraw gate never compares — so with an HR tile pinned, the tile froze until something
    /// *else* (a moving fix, reopening the screen) happened to repaint. Pins the
    /// `prev_live_sensors` edge: a changed displayed value dirties the riding view exactly once,
    /// an unchanged one doesn't, and the 5 s staleness expiry (the blank to `--`) is an edge too.
    #[test]
    fn fresh_sensor_sample_repaints_the_riding_view() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // stack [Home, Map] — a riding view
        assert!(app.settings.stat_fields.push(crate::stat_fields::StatField::HeartRate));
        let _ = app.take_dirty(); // drain the boot repaint

        tick_hr_only(&mut app, Some(155), 1_000);
        assert!(app.take_dirty().map, "a fresh HR sample must repaint the riding view");

        // A new sample with the same displayed value is not an edge.
        tick_hr_only(&mut app, Some(155), 2_000);
        assert!(!app.take_dirty().map, "an unchanged displayed value must not re-dirty");

        tick_hr_only(&mut app, Some(156), 3_000);
        assert!(app.take_dirty().map, "a changed bpm repaints again");

        // The strap drops: >5 s later the staleness gate blanks the tile — that flip must paint
        // (once), or the rider stares at a frozen last value.
        tick_hr_only(&mut app, None, 9_001);
        assert!(app.take_dirty().map, "the staleness expiry (value → `--`) must repaint");
        tick_hr_only(&mut app, None, 20_000);
        assert!(!app.take_dirty().map, "still blank → no re-dirty");
    }

    /// The economy half of the SR3 edge: with **no sensor tile pinned** (the default six fields), a
    /// notification stream must never force map renders — the same render-on-demand economy the
    /// battery / `temp_c` gates keep.
    #[test]
    fn sensor_sample_without_a_pinned_tile_never_repaints() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let _ = app.take_dirty();
        tick_hr_only(&mut app, Some(155), 1_000);
        assert!(!app.take_dirty().map, "no HR tile pinned → an HR sample must not force a render");
    }

    /// And off the riding views entirely (Home is the base), a pinned tile still doesn't repaint —
    /// nothing on Home draws it; entering Statistics repaints on the screen change anyway.
    #[test]
    fn sensor_sample_on_home_never_repaints() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0)); // base = Home
        assert!(app.settings.stat_fields.push(crate::stat_fields::StatField::HeartRate));
        let _ = app.take_dirty();
        tick_hr_only(&mut app, Some(155), 1_000);
        assert!(!app.take_dirty().map, "Home draws no tiles → no repaint for a sample");
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
    /// Home it's the wall-clock minute boundary; on a static menu the idle-return timeout is the
    /// only pending wake (the menu itself animates on nothing). With the idle return disabled a
    /// static menu reports `None` — sleep until input.
    #[test]
    fn ms_until_next_wake_reports_the_home_minute_then_the_idle_deadline_on_a_static_menu() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0)); // base = Home
        app.set_settings(crate::settings::Settings {
            clock: DateTime { year: 2025, month: 1, day: 1, hour: 12, minute: 0 },
            idle_return: crate::settings::IdleReturn::Never, // isolate the clock deadline first
            ..Default::default()
        });
        // Home shows a clock → the deadline is the time left until the displayed minute rolls over.
        app.advance_animations(InputClock(0));
        assert_eq!(app.ms_until_next_wake(0), Some(60_000), "at a boundary the whole minute remains");
        app.advance_animations(InputClock(25_000));
        assert_eq!(app.ms_until_next_wake(25_000), Some(35_000), "25 s in, 35 s until the next repaint");
        // Navigate to the static Menu (BackHold): with the idle return off, it animates on nothing,
        // so there is no deadline — the host sleeps until the next input or sensor event.
        app.apply_gesture(Gesture::BackHold);
        app.advance_animations(InputClock(25_000));
        assert_eq!(app.ms_until_next_wake(25_000), None, "a static menu with idle-return off needs no timed wake");
        // Turn the idle return on: the static menu now reports the idle-return deadline as its wake.
        // The BackHold that opened the menu was the last input (at 25 s), so a full 30 s window
        // remains at 25 s.
        app.settings.idle_return = crate::settings::IdleReturn::S30;
        app.advance_animations(InputClock(25_000));
        assert_eq!(app.ms_until_next_wake(25_000), Some(30_000), "the idle-return timeout is the pending wake");
    }

    // --- climb state tracking (C3, #509) ---
    //
    // Two layers, matching the code split: the **pure** hysteresis resolver
    // (`resolve_active_climb`) is pinned directly over a hand-built `Climbs` list — enter, exit,
    // and the flap-guard — with no reader; then the wiring (build-on-load, clear-on-unload, and the
    // once-per-entry `ClimbProfile::fill`) is driven end-to-end through `App::update_active_climb`
    // and `App::tick` over the committed `grimsel-climb.obcr` fixture (3 back-to-back climbs).

    // `Climbs` / `RouteReader` are already in scope via `use super::*`; only the extras the
    // fixture tests need are imported here.
    use obc_route::{ClimbSeg, RouteIndex, SliceSource, WptEntry};

    /// A `ClimbSeg` over `[start_m, end_m]` — the other fields don't affect the interval hysteresis.
    fn seg(start_m: u32, end_m: u32) -> ClimbSeg {
        ClimbSeg {
            start_m,
            end_m,
            base_ele_m: 0,
            top_ele_m: (end_m - start_m) as i16,
            gain_m: (end_m - start_m) as u16,
            avg_grade_pct: 5,
            category: 0,
        }
    }

    /// A `Climbs` list from `(start, end)` pairs.
    fn climbs(spans: &[(u32, u32)]) -> Climbs {
        let mut c = Climbs::new();
        for &(s, e) in spans {
            c.0.push(seg(s, e)).unwrap();
        }
        c
    }

    /// Enter: below a climb's entry band there's no active climb; once progress reaches within
    /// `CLIMB_ENTER_MARGIN_M` of the base the climb arms (slightly *before* the base), and it stays
    /// armed through the interval.
    #[test]
    fn resolve_arms_a_climb_at_its_entry_band() {
        let cs = climbs(&[(1000, 3000)]);
        // Well before the entry band (base 1000 − 50 = 950): nothing.
        assert_eq!(resolve_active_climb(&cs, 800, None), None);
        // Just outside the band: still nothing.
        assert_eq!(resolve_active_climb(&cs, 949, None), None);
        // Inside the entry band, before the base: armed early (the point of the enter margin).
        assert_eq!(resolve_active_climb(&cs, 960, None), Some(0));
        // Mid-climb: on it.
        assert_eq!(resolve_active_climb(&cs, 2000, None), Some(0));
    }

    /// Exit: while on a climb it's *held* past the summit by `CLIMB_EXIT_MARGIN_M`, then disarms.
    #[test]
    fn resolve_holds_past_the_summit_then_exits() {
        let cs = climbs(&[(1000, 3000)]);
        // At the summit: still on it.
        assert_eq!(resolve_active_climb(&cs, 3000, Some(0)), Some(0));
        // Within the exit band (summit 3000 + 30 = 3030): held.
        assert_eq!(resolve_active_climb(&cs, 3025, Some(0)), Some(0));
        // Past the exit band: disarmed (no next climb to take over).
        assert_eq!(resolve_active_climb(&cs, 3040, Some(0)), None);
    }

    /// The flap guard: jitter around the base boundary (the matcher wobbling progress a few metres
    /// either way of the entry point) must not toggle the active climb once it's armed.
    #[test]
    fn resolve_does_not_flap_at_a_boundary() {
        let cs = climbs(&[(1000, 3000)]);
        // Arm at the base.
        let mut active = resolve_active_climb(&cs, 1000, None);
        assert_eq!(active, Some(0));
        // Progress jitters back a few metres below the base across several fixes — inside the entry
        // band, so the climb stays armed every time (no off→on→off flapping).
        for p in [995u32, 980, 970, 990, 1005, 998] {
            active = resolve_active_climb(&cs, p, active);
            assert_eq!(active, Some(0), "jitter around the base must not drop the active climb");
        }
        // …and jitter around the *summit* likewise doesn't flap (held by the exit band).
        active = resolve_active_climb(&cs, 3000, active);
        for p in [3005u32, 2998, 3010, 2995, 3020] {
            active = resolve_active_climb(&cs, p, active);
            assert_eq!(active, Some(0), "jitter around the summit must not drop the active climb");
        }
    }

    /// Back-to-back climbs (the Grimsel shape): leaving climb 0's exit band hands straight over to
    /// climb 1 whose entry band it's already inside — one clean transition, never a gap of `None`.
    #[test]
    fn resolve_hands_over_between_adjacent_climbs() {
        let cs = climbs(&[(1000, 3000), (3000, 5000)]);
        // On climb 0 at its summit, held through the exit band.
        assert_eq!(resolve_active_climb(&cs, 3010, Some(0)), Some(0));
        // Past climb 0's exit band: re-arms, and climb 1's entry band already contains progress →
        // straight onto climb 1.
        assert_eq!(resolve_active_climb(&cs, 3040, Some(0)), Some(1));
    }

    /// A stale index (the list shrank under the previous active climb, e.g. a swap to a flatter
    /// route) doesn't strand the resolver — it re-arms from scratch (here: nothing).
    #[test]
    fn resolve_recovers_from_a_stale_index() {
        let cs = climbs(&[(1000, 3000)]);
        // prev = 5, but only one climb exists and progress is nowhere near it.
        assert_eq!(resolve_active_climb(&cs, 200, Some(5)), None);
    }

    // --- next-waypoint tracking (#569) ---
    //
    // The pure resolver `resolve_next_waypoint` is pinned directly over a hand-built `Waypoints`
    // table: the linger advance, the anti-flap jitter guard, the past-the-last `None`, and a fresh
    // route starting at index 0. (The App-side wiring — build-on-load, off-route freeze, re-window,
    // route-swap clear — rides the same `tick`/`Activity` machinery the climb wiring does.)

    /// A `Waypoints` table from `(dist_along_m, name)` pairs, in route order.
    fn wpts(items: &[(u32, &str)]) -> Waypoints {
        let mut w = Waypoints::new();
        for &(dist_along_m, name) in items {
            let mut n = heapless::String::new();
            n.push_str(name).unwrap();
            w.entries.push(WptEntry { dist_along_m, lon: 0, lat: 0, name: n }).unwrap();
        }
        w
    }

    /// The index advances at exactly `dist + WAYPOINT_LINGER_M`, and not one metre before — the
    /// passed waypoint lingers the whole 100 m band.
    #[test]
    fn resolve_next_advances_exactly_at_the_linger() {
        let w = wpts(&[(1_000, "A"), (2_000, "B")]);
        // Before A, and anywhere in A's linger band [1000, 1100): A is next.
        assert_eq!(resolve_next_waypoint(&w, 0, None), Some(0));
        assert_eq!(resolve_next_waypoint(&w, 1_000, None), Some(0));
        assert_eq!(resolve_next_waypoint(&w, 1_099, Some(0)), Some(0));
        // Exactly at dist + 100: A's band closes, B is next.
        assert_eq!(resolve_next_waypoint(&w, 1_100, Some(0)), Some(1));
    }

    /// Jitter around a waypoint's own position (progress wobbling ±30 m across A's `dist`) never
    /// flaps the index — the linger band absorbs it.
    #[test]
    fn resolve_next_does_not_flap_around_a_waypoint() {
        let w = wpts(&[(1_000, "A"), (2_000, "B")]);
        let mut next = resolve_next_waypoint(&w, 970, None);
        assert_eq!(next, Some(0));
        for p in [1_005u32, 980, 1_030, 995, 1_020, 970] {
            next = resolve_next_waypoint(&w, p, next);
            assert_eq!(next, Some(0), "jitter around A's position must not advance the index");
        }
        // …and a dip back below the advance boundary after passing it doesn't regress the index.
        next = resolve_next_waypoint(&w, 1_100, next);
        assert_eq!(next, Some(1));
        for p in [1_080u32, 1_060, 1_090] {
            next = resolve_next_waypoint(&w, p, next);
            assert_eq!(next, Some(1), "a progress dip must not step back onto a passed waypoint");
        }
    }

    /// Past the last waypoint's linger the index is `None` — the chip / fields go empty.
    #[test]
    fn resolve_next_is_none_past_the_last() {
        let w = wpts(&[(1_000, "A"), (2_000, "B")]);
        // Inside B's band: still B.
        assert_eq!(resolve_next_waypoint(&w, 2_099, Some(1)), Some(1));
        // Past B + 100: nothing ahead.
        assert_eq!(resolve_next_waypoint(&w, 2_100, Some(1)), None);
        assert_eq!(resolve_next_waypoint(&w, 9_999, Some(1)), None);
    }

    /// A fresh route (no prior index) starts at the first waypoint ahead — index 0 from progress 0,
    /// or the first still-ahead one when the rider starts mid-route.
    #[test]
    fn resolve_next_fresh_route_starts_at_the_first_ahead() {
        let w = wpts(&[(1_000, "A"), (2_000, "B"), (3_000, "C")]);
        assert_eq!(resolve_next_waypoint(&w, 0, None), Some(0));
        // Starting past A's linger picks B (the first still-ahead), not A.
        assert_eq!(resolve_next_waypoint(&w, 1_500, None), Some(1));
        // An empty table is always `None`.
        assert_eq!(resolve_next_waypoint(&Waypoints::new(), 0, None), None);
    }

    /// The committed Grimsel fixture bytes (3 back-to-back climbs), embedded so the `no_std` lib
    /// tests need no `std::fs`. Boundaries: 501–11067, 11067–14472, 14472–18547; total ~18.7 km.
    const GRIMSEL: &[u8] = include_bytes!("../../obc-sim/assets/grimsel-climb.obcr");

    /// Parse the fixture into a `RouteIndex` the callers pair with a `SliceSource` over [`GRIMSEL`].
    fn grimsel_index() -> RouteIndex {
        let src = SliceSource(GRIMSEL);
        RouteIndex::read(&src).unwrap()
    }

    /// Drive the active-climb state directly through `App::update_active_climb` with a controlled
    /// `progress_m`, over the real fixture reader — isolating the hysteresis + once-per-entry refill
    /// from the matcher's fix-snapping (which can't place progress to the metre).
    #[test]
    fn update_active_climb_refills_exactly_on_entry_transitions() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let idx = grimsel_index();
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);
        app.climbs = route.detect_climbs();
        assert_eq!(app.climbs.len(), 3, "the Grimsel fixture segments into 3 climbs");

        // Sweep progress across the whole route in 250 m steps. Climb boundaries (from the fixture):
        // 501–11067, 11067–14472, 14472–18547 — three entries as the sweep crosses each base.
        let mut entries = 0;
        let mut prev = None;
        for p in (0..=18_725u32).step_by(250) {
            app.activity.progress_m = p;
            app.update_active_climb(&route);
            if app.activity.active_climb != prev && app.activity.active_climb.is_some() {
                entries += 1;
            }
            prev = app.activity.active_climb;
        }
        // Exactly one refill per climb *entry* — never per fix on the same climb. Three climbs, and
        // because they're back-to-back the sweep enters all three: 3 entries ⇒ 3 fills.
        assert_eq!(entries, 3, "the sweep enters each of the 3 climbs once");
        assert_eq!(app.climb_fill_count, 3, "the detail buffer is rebuilt exactly on the 3 entries, not per fix");
    }

    /// Off-route freezes the active climb: a stale (frozen) match must not strand the rider onto a
    /// climb, nor drop the one they were on — the state holds until they rejoin and progress moves.
    #[test]
    fn update_active_climb_freezes_while_off_route() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let idx = grimsel_index();
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);
        app.climbs = route.detect_climbs();

        // On climb 0 (progress mid-first-climb).
        app.activity.progress_m = 5000;
        app.update_active_climb(&route);
        assert_eq!(app.activity.active_climb, Some(0));
        let fills_on_climb = app.climb_fill_count;

        // Go off-route: progress freezes (the matcher holds it). Even a progress value that would
        // otherwise be past every climb must not change the active climb while off-route.
        app.activity.off_route = true;
        app.activity.progress_m = 99_999;
        app.update_active_climb(&route);
        assert_eq!(app.activity.active_climb, Some(0), "off-route holds the current climb");
        assert_eq!(app.climb_fill_count, fills_on_climb, "no refill while off-route");
    }

    // --- host auto-switch / auto-return (C5, #511) ---
    //
    // Driven off the same climb entry/exit edge in `update_active_climb`, so these tests reuse the
    // Grimsel fixture and step `progress_m` across a base / summit to fire the transition, then
    // inspect which screen is on top. `App::new` gives stack `[Home, Map]` — top = Map, a riding view.

    /// A Riding-view app with the fixture's climbs loaded and a given climb mode — the common
    /// setup for the auto-switch cases below.
    fn climb_app(mode: crate::settings::ClimbMode) -> (App, RouteIndex) {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // stack [Home, Map], Riding
        app.settings.climb_mode = mode;
        let idx = grimsel_index();
        {
            let src = SliceSource(GRIMSEL);
            let route = RouteReader::new(&idx, &src);
            app.climbs = route.detect_climbs();
        }
        (app, idx)
    }

    /// Enter a climb (drive progress across climb 0's base) via `update_active_climb`.
    fn enter_first_climb(app: &mut App, idx: &RouteIndex) {
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(idx, &src);
        app.activity.progress_m = 5_000; // mid climb 0 (501–11067)
        app.update_active_climb(&route);
        assert_eq!(app.activity.active_climb, Some(0), "the fixture puts progress on climb 0");
    }

    /// Auto + on a riding view: entering a climb auto-switches the top to the Climb screen.
    #[test]
    fn auto_switches_to_climb_on_entry_from_a_riding_view() {
        use crate::settings::ClimbMode;
        let (mut app, idx) = climb_app(ClimbMode::Auto);
        assert!(matches!(app.top_screen(), Screen::Map(_)), "starts on the Map (a riding view)");
        enter_first_climb(&mut app, &idx);
        assert!(matches!(app.top_screen(), Screen::Climb(_)), "Auto auto-shows the Climb screen on entry");
    }

    /// The menu guard: the rider deep in a menu (a non-riding view on top) is never yanked onto the
    /// Climb screen, even in Auto — the switch only fires from a riding view.
    #[test]
    fn auto_never_switches_away_from_a_menu() {
        use crate::screen::{MenuScreen, ScreenKind};
        use crate::settings::ClimbMode;
        let (mut app, idx) = climb_app(ClimbMode::Auto);
        // Open the Menu over the Map (a Nav-kind screen on top).
        let _ = app.stack.push(Screen::Menu(MenuScreen::new()));
        assert_ne!(app.top_screen().kind(), ScreenKind::Riding, "top is now a menu, not a riding view");
        enter_first_climb(&mut app, &idx);
        assert!(matches!(app.top_screen(), Screen::Menu(_)), "the menu is left untouched by the entry edge");
        // And the map underneath it is still the Map — the switch didn't reach past the menu.
        assert!(matches!(app.stack[app.stack.len() - 2], Screen::Map(_)), "the base riding view is untouched too");
    }

    /// Manual and Off never auto-switch on entry (the rider reaches the Climb screen only by cycling
    /// Back, or not at all).
    #[test]
    fn manual_and_off_never_auto_switch_on_entry() {
        use crate::settings::ClimbMode;
        for mode in [ClimbMode::Manual, ClimbMode::Off] {
            let (mut app, idx) = climb_app(mode);
            enter_first_climb(&mut app, &idx);
            assert!(matches!(app.top_screen(), Screen::Map(_)), "{mode:?} leaves the rider on the Map on entry");
        }
    }

    /// Crest auto-return: from the Climb screen, ending the climb (progress past the exit band)
    /// returns to the Map — a stale "No climb" panel is never left up.
    #[test]
    fn crest_auto_returns_to_map_from_the_climb_screen() {
        use crate::settings::ClimbMode;
        let (mut app, idx) = climb_app(ClimbMode::Auto);
        enter_first_climb(&mut app, &idx); // Auto → now on the Climb screen
        assert!(matches!(app.top_screen(), Screen::Climb(_)));
        // Jump progress past the last climb's exit band so the active climb clears (Some → None).
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);
        app.activity.progress_m = 50_000;
        app.update_active_climb(&route);
        assert_eq!(app.activity.active_climb, None, "past every climb → no active climb");
        assert!(matches!(app.top_screen(), Screen::Map(_)), "the crest returns to the Map from the Climb screen");
    }

    /// The crest return only touches the Climb screen: if the rider is on some other view when the
    /// climb ends, that view is left as-is (never force-switched to the Map).
    #[test]
    fn crest_leaves_other_screens_untouched() {
        use crate::screen::MenuScreen;
        use crate::settings::ClimbMode;
        let (mut app, idx) = climb_app(ClimbMode::Manual); // Manual: entry won't switch
        enter_first_climb(&mut app, &idx);
        let _ = app.stack.push(Screen::Menu(MenuScreen::new())); // now on a menu, mid-climb
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);
        app.activity.progress_m = 50_000;
        app.update_active_climb(&route);
        assert_eq!(app.activity.active_climb, None);
        assert!(matches!(app.top_screen(), Screen::Menu(_)), "a crest never yanks a menu to the Map");
    }

    /// Build-on-load / clear-on-unload wiring through `tick`: an active route with a reader segments
    /// the climbs once; dropping the route (active_route → None) clears the list and the on-climb
    /// state. Uses `tick` (not the internal setter) to exercise the real load/unload path.
    #[test]
    fn tick_builds_climbs_on_load_and_clears_on_unload() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // map-first, Riding
        let idx = grimsel_index();
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);

        // No route active yet → tick with a reader builds nothing (active_route is None).
        let no_loc = |app: &mut App, route: Option<&RouteReader>| {
            let mut loc = OneFix(None);
            app.tick(
                RideClock(0),
                Sensors {
                    loc: &mut loc,
                    altimeter: None,
                    temperature: None,
                    clock: None,
                    compass: None,
                    track: None,
                    fuel: None,
                    hr: None,
                    power: None,
                    cadence: None,
                },
                route,
            );
        };
        no_loc(&mut app, Some(&route));
        assert!(app.climbs.is_empty(), "no active route → no climbs, even with a reader present");
        assert!(app.climbs_route.is_none());
        assert!(app.waypoints.is_empty() && app.waypoints_route.is_none(), "no active route → no waypoint table");

        // Load the route (active_route = Some) and tick with the reader → climbs segmented once, and
        // the waypoint table loaded on the same edge (GRIMSEL carries none, so the table is empty but
        // the build key advances to Some(0) — the load ran).
        app.activity.active_route = Some(0);
        no_loc(&mut app, Some(&route));
        assert_eq!(app.climbs.len(), 3, "an active route + reader segments the climbs on load");
        assert_eq!(app.climbs_route, Some(0));
        assert_eq!(app.waypoints_route, Some(0), "the waypoint table loads on the same route edge");

        // Unload (active_route → None) and tick → the climbs / waypoints and their derived indices clear.
        app.activity.active_climb = Some(0); // pretend we were on a climb
        app.activity.next_waypoint = Some(0); // …and had a next waypoint
        app.activity.active_route = None;
        no_loc(&mut app, None);
        assert!(app.climbs.is_empty(), "unloading the route clears the climbs");
        assert!(app.climbs_route.is_none());
        assert_eq!(app.activity.active_climb, None, "and the on-climb state is dropped");
        assert!(app.waypoints.is_empty() && app.waypoints_route.is_none(), "unloading clears the waypoint table");
        assert_eq!(app.activity.next_waypoint, None, "and the next-waypoint index is dropped");
    }

    // --- idle-return timeout (Part B) ---
    //
    // The idle sweep runs in `advance_animations`; these tests set `last_input_ms`, push a screen,
    // then advance the clock past the deadline and inspect the top screen. `App::new` starts on the
    // Map (Riding, a tracking session isn't armed until `start_session`); `new_idle` starts on Home.

    use crate::screen::{
        MenuScreen, NavPlanningScreen, PasskeyScreen, RouteReceivedScreen, SettingsScreen, StatisticsScreen,
        WarningFlags, WarningScreen,
    };
    use crate::settings::IdleReturn;

    /// Run one idle sweep at `now_ms` — the same path `advance_animations` takes, at a chosen clock.
    fn idle_tick(app: &mut App, now_ms: u32) {
        app.advance_animations(InputClock(now_ms));
    }

    /// Not tracking: after the timeout with no input, any screen clears to the Home root.
    #[test]
    fn idle_returns_to_home_when_not_tracking() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0)); // [Home], Idle
        app.settings.idle_return = IdleReturn::S30;
        let _ = app.stack.push(Screen::Menu(MenuScreen::new()));
        let _ = app.stack.push(Screen::Settings(SettingsScreen::new()));
        app.last_input_ms = 0;

        idle_tick(&mut app, 29_000); // still inside the window
        assert!(matches!(app.top_screen(), Screen::Settings(_)), "no return before the deadline");

        idle_tick(&mut app, 30_000); // deadline reached
        assert_eq!(app.stack.len(), 1, "cleared to the Home root");
        assert!(matches!(app.top_screen(), Screen::Home(_)), "and the top is Home");
    }

    /// Returning to Home reseeds the screensaver backdrop, exactly as a manual return does.
    #[test]
    fn idle_return_home_reseeds_the_backdrop() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.settings.idle_return = IdleReturn::S15;
        let _ = app.stack.push(Screen::Menu(MenuScreen::new()));
        app.last_input_ms = 0;
        idle_tick(&mut app, 20_000);
        let Some(Screen::Home(home)) = app.stack.first() else { panic!("back on Home") };
        assert_eq!(home.seed(), 20_000, "the backdrop reseeds to the return's clock");
    }

    /// Tracking: a menu screen returns to the Map; the deliberate ride views do not time out.
    #[test]
    fn idle_returns_to_map_when_tracking_from_a_menu() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map], Riding
        app.activity.start_session(); // arm a tracking session
        app.settings.idle_return = IdleReturn::S30;
        let _ = app.stack.push(Screen::Menu(MenuScreen::new()));
        app.last_input_ms = 0;

        idle_tick(&mut app, 30_000);
        assert!(matches!(app.top_screen(), Screen::Map(_)), "a menu times out to the Map mid-ride");
        assert_eq!(app.stack.len(), 2, "landed on [Home, Map], not deeper");
    }

    /// The ride views (Map, Statistics, Climb, RideControl) never time out while tracking.
    #[test]
    fn ride_views_never_time_out_while_tracking() {
        for view in [
            Screen::Map(MapScreen::new()),
            Screen::Statistics(StatisticsScreen::new()),
            Screen::RideControl(crate::screen::RideControl::new()),
        ] {
            let mut app = App::new(AppState::new(0, 0, 1.0));
            app.activity.start_session();
            app.settings.idle_return = IdleReturn::S15;
            *app.stack.last_mut().unwrap() = view; // replace the base Map with the view under test
            let kind_before = core::mem::discriminant(app.top_screen());
            app.last_input_ms = 0;
            idle_tick(&mut app, 60_000);
            assert_eq!(core::mem::discriminant(app.top_screen()), kind_before, "a ride view is left put");
        }
    }

    /// The modal cards (passkey, route popups, the #504 warning card) and the planning spinner are
    /// exempt — never yanked by the idle sweep. Elapse to 20 s (past the 15 s idle deadline, but
    /// under the route popup's own 30 s auto-close, so only the idle exemption is under test here).
    #[test]
    fn modal_cards_are_exempt_from_idle_return() {
        for card in [
            Screen::Passkey(PasskeyScreen::new(123_456)),
            Screen::RouteReceived(RouteReceivedScreen::new(0, 0, None)),
            Screen::NavPlanning(NavPlanningScreen::new("Route")),
            Screen::Warning(WarningScreen::new(WarningFlags::NO_GPS)),
        ] {
            let mut app = App::new_idle(AppState::new(0, 0, 1.0));
            app.settings.idle_return = IdleReturn::S15;
            let kind = core::mem::discriminant(&card);
            let _ = app.stack.push(card);
            app.last_input_ms = 0;
            idle_tick(&mut app, 20_000);
            assert_eq!(core::mem::discriminant(app.top_screen()), kind, "the modal card stays up");
        }
    }

    /// The route-less **browse map** (Map on top, not tracking — Menu → Map) is a deliberate view,
    /// so it's exempt from the idle-return timeout even though it isn't the Home root: elapse well
    /// past the deadline and it stays put (unlike a menu, which would return to Home).
    #[test]
    fn browse_map_is_exempt_from_idle_return() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0)); // Idle, not tracking
        app.settings.idle_return = IdleReturn::S15;
        let _ = app.stack.push(Screen::Map(MapScreen::new())); // the browse map over Home
        app.last_input_ms = 0;
        idle_tick(&mut app, 60_000);
        assert!(matches!(app.top_screen(), Screen::Map(_)), "the browse map is a deliberate view — never yanked");
        // The browse map's only pending wake is the one-shot start hint's auto-hide (T6, #684); once
        // that window has elapsed it arms no wake at all — in particular no idle-return wake.
        idle_tick(&mut app, 60_000 + 4_000);
        assert_eq!(app.ms_until_next_wake(60_000 + 4_000), None, "and it arms no idle wake");

        // A menu over Home, by contrast, does return.
        *app.stack.last_mut().unwrap() = Screen::Menu(MenuScreen::new());
        app.last_input_ms = 60_000;
        idle_tick(&mut app, 120_000);
        assert!(matches!(app.top_screen(), Screen::Home(_)), "a menu still returns to Home on the timeout");
    }

    /// Any gesture resets the idle deadline — a turn 1 ms before it would fire buys another full window.
    #[test]
    fn a_gesture_resets_the_idle_deadline() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.settings.idle_return = IdleReturn::S30;
        let _ = app.stack.push(Screen::Menu(MenuScreen::new()));
        app.last_input_ms = 0;

        // A gesture at 29 s (just shy of the deadline) resets the clock.
        app.now_ms = 29_000;
        app.apply_gesture(Gesture::Turn(1));
        assert_eq!(app.last_input_ms, 29_000, "the gesture reset the idle clock");

        idle_tick(&mut app, 30_000); // 1 s after the gesture — well inside the fresh window
        assert!(matches!(app.top_screen(), Screen::Menu(_)), "the reset deadline hasn't elapsed");

        idle_tick(&mut app, 59_000); // 30 s after the gesture
        assert!(matches!(app.top_screen(), Screen::Home(_)), "and now it fires");
    }

    /// `Never` disables the mechanism entirely — no return however long the device idles, and no
    /// idle wake is armed.
    #[test]
    fn never_disables_the_idle_return() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.settings.idle_return = IdleReturn::Never;
        let _ = app.stack.push(Screen::Menu(MenuScreen::new()));
        app.last_input_ms = 0;
        idle_tick(&mut app, 10 * 60_000); // ten minutes
        assert!(matches!(app.top_screen(), Screen::Menu(_)), "Never never returns");
        assert_eq!(app.ms_until_next_wake(10 * 60_000), None, "and arms no idle wake");
    }

    /// The idle deadline is folded into the host's wake so a parked device wakes to return.
    #[test]
    fn idle_return_arms_a_wake() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.settings.idle_return = IdleReturn::S30;
        let _ = app.stack.push(Screen::Menu(MenuScreen::new()));
        app.last_input_ms = 0;
        idle_tick(&mut app, 10_000);
        assert_eq!(app.ms_until_next_wake(10_000), Some(20_000), "wake armed 20 s out (30 s − 10 s elapsed)");
    }

    /// The DFU one-shots (epic #615 S4/S5): the install request and the confirmed-update fact are
    /// both drained exactly once — the create-route request contract. `request_dfu_install` (the
    /// `dfu-install` debug path) posts the [`DfuAction::Install`] the board's drain matches on.
    #[test]
    fn dfu_request_and_confirmed_fact_are_take_once() {
        use crate::activity::DfuAction;
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        assert_eq!(app.take_dfu_request(), None, "nothing pending at boot");
        app.request_dfu_install();
        assert_eq!(app.take_dfu_request(), Some(DfuAction::Install), "the posted request drains");
        assert_eq!(app.take_dfu_request(), None, "…exactly once");

        assert_eq!(app.take_update_confirmed(), None, "no confirmed update on a normal boot");
        app.notify_update_confirmed("v1.2.3-4-gabc1234");
        let v = app.take_update_confirmed().expect("the fact is set");
        assert_eq!(v.as_str(), "v1.2.3-4-gabc1234");
        assert_eq!(app.take_update_confirmed(), None, "taken once — the toast shows once");
    }

    /// The S5 scan-result seam (epic #615 S5, #620): `notify_dfu_scan_result` lands in the
    /// "Checking card..." wait the System menu pushed, swapping it for the confirm screen (`Ok`) or
    /// the error card (`Err`); with no wait on the stack it's a no-op (the rider pressed Back).
    #[test]
    fn dfu_scan_result_replaces_the_check_wait() {
        use crate::dfu::{DfuScanError, DfuScanReport};
        let mk = |v: &str| {
            let mut s = heapless::String::new();
            let _ = s.push_str(v);
            s
        };
        let report =
            DfuScanReport { installed: mk("v1.0.0-0-gaaa"), staged: mk("v1.1.0-3-gbbb"), first_install: false };

        // No wait up → dropped.
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.notify_dfu_scan_result(Ok(report.clone()));
        assert!(!app.stack.iter().any(|s| matches!(s, Screen::DfuConfirm(_))), "no wait ⇒ answer dropped");

        // Wait up → Ok swaps in the confirm.
        let _ = app.stack.push(Screen::DfuCheck(crate::screen::DfuCheckScreen::new()));
        app.notify_dfu_scan_result(Ok(report));
        assert!(matches!(app.top_screen(), Screen::DfuConfirm(_)), "Ok swaps the wait for the confirm");

        // Wait up → Err swaps in the error card, carrying the variant.
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let _ = app.stack.push(Screen::DfuCheck(crate::screen::DfuCheckScreen::new()));
        app.notify_dfu_scan_result(Err(DfuScanError::TooFragmented));
        match app.top_screen() {
            Screen::DfuError(e) => assert_eq!(e.error(), DfuScanError::TooFragmented),
            _ => panic!("Err swaps the wait for the error card"),
        }
    }

    /// The S6 remote-check seam (epic #615 S6, #621): a BLE `installFw` opens the **same** scan →
    /// confirm flow the System menu's press does — push the DfuCheck wait + post
    /// [`DfuAction::Scan`], never `Install` — exactly once per accepted call.
    #[test]
    fn remote_dfu_check_opens_scan_flow_once() {
        use crate::activity::DfuAction;
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        assert!(app.open_remote_dfu_check(), "an idle app opens the flow");
        let checks = app.stack.iter().filter(|s| matches!(s, Screen::DfuCheck(_))).count();
        assert_eq!(checks, 1, "exactly one wait screen pushed");
        assert_eq!(app.take_dfu_request(), Some(DfuAction::Scan), "a Scan is posted — NEVER Install");
        assert_eq!(app.take_dfu_request(), None, "…exactly once");
    }

    /// Remote-check deferral behind the passkey card (S6, #621): the request is *deferred*, not
    /// dropped — `open_remote_dfu_check` returns `false` (the board keeps its pending flag and
    /// retries), posts nothing, pushes nothing; once the card clears, the same call opens the flow.
    #[test]
    fn remote_dfu_check_defers_behind_the_passkey_card() {
        use crate::activity::DfuAction;
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let _ = app.stack.push(Screen::Passkey(crate::screen::PasskeyScreen::new(123_456)));
        assert!(!app.open_remote_dfu_check(), "deferred while the pairing code shows");
        assert!(!app.stack.iter().any(|s| matches!(s, Screen::DfuCheck(_))), "nothing pushed");
        assert_eq!(app.take_dfu_request(), None, "nothing posted");
        // The card clears (pairing completed/failed) → the retried drain opens the flow.
        app.stack.pop();
        assert!(app.open_remote_dfu_check(), "opens once the card cleared");
        assert!(matches!(app.top_screen(), Screen::DfuCheck(_)));
        assert_eq!(app.take_dfu_request(), Some(DfuAction::Scan));
    }

    /// Remote-check never double-opens (S6, #621): while any DFU screen is on the stack — the wait
    /// a previous call (or the rider's own menu press) pushed, or the confirm it swapped into — a
    /// further remote request defers rather than stacking a second flow. Recording defers too
    /// (defensive: the BLE edge answers `busy`, but recording can start between reply and drain).
    #[test]
    fn remote_dfu_check_never_double_pushes_and_defers_while_recording() {
        use crate::activity::DfuAction;
        // A remote-opened flow blocks a second remote open — even after its Scan drained.
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        assert!(app.open_remote_dfu_check());
        assert!(!app.open_remote_dfu_check(), "undrained Scan + wait screen ⇒ deferred");
        assert_eq!(app.take_dfu_request(), Some(DfuAction::Scan), "the one Scan");
        assert!(!app.open_remote_dfu_check(), "wait screen still up ⇒ still deferred");
        assert_eq!(app.stack.iter().filter(|s| matches!(s, Screen::DfuCheck(_))).count(), 1);

        // The rider's own confirm screen (menu-opened flow) blocks a remote open the same way.
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let mk = |v: &str| {
            let mut s = heapless::String::new();
            let _ = s.push_str(v);
            s
        };
        let report = crate::dfu::DfuScanReport { installed: mk("v1"), staged: mk("v2"), first_install: false };
        let _ = app.stack.push(Screen::DfuConfirm(crate::screen::DfuConfirmScreen::new(report)));
        assert!(!app.open_remote_dfu_check(), "a confirm on the stack ⇒ deferred, never yanked");
        assert_eq!(app.take_dfu_request(), None);

        // Recording defers (the arm ends in a reboot — a live ride would be lost).
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.activity.start_session();
        assert!(!app.open_remote_dfu_check(), "deferred while recording");
        assert_eq!(app.take_dfu_request(), None);
    }

    /// The post-update toast (epic #615 S5): a confirmed-update fact surfaces the "Updated to vX"
    /// card once on the next `advance_animations` pass; a normal boot (no fact) pushes nothing.
    #[test]
    fn confirmed_update_pushes_the_toast_once() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.advance_animations(InputClock(1000));
        assert!(!app.stack.iter().any(|s| matches!(s, Screen::DfuUpdated(_))), "a normal boot shows no toast");

        app.notify_update_confirmed("v2.0.0-0-gccc");
        app.advance_animations(InputClock(2000));
        assert!(matches!(app.top_screen(), Screen::DfuUpdated(_)), "the confirmed update surfaces the toast");
        app.stack.pop(); // dismiss
        app.advance_animations(InputClock(3000));
        assert!(!app.stack.iter().any(|s| matches!(s, Screen::DfuUpdated(_))), "shown once — the fact was consumed");
    }

    /// The failure twin: a failed-update fact surfaces the "UPDATE FAILED" card once — with the
    /// typed verdict the seam carries — and a normal boot pushes nothing.
    #[test]
    fn failed_update_pushes_the_card_once() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.advance_animations(InputClock(1000));
        assert!(!app.stack.iter().any(|s| matches!(s, Screen::DfuFailed(_))), "a normal boot shows no failure card");

        app.notify_update_failed(crate::dfu::DfuFailure::Reverted, Some("v2.0.0-0-gccc"));
        app.advance_animations(InputClock(2000));
        match app.top_screen() {
            Screen::DfuFailed(card) => assert_eq!(card.why(), crate::dfu::DfuFailure::Reverted),
            _ => panic!("expected the failure card on top"),
        }
        app.stack.pop(); // dismiss
        app.advance_animations(InputClock(3000));
        assert!(!app.stack.iter().any(|s| matches!(s, Screen::DfuFailed(_))), "shown once — the fact was consumed");
    }

    /// The Ride detail's track-request seam (#680): no request without an open detail; an open one
    /// hands out the viewed ride's **durable id** and re-polls until answered; the host's answer
    /// (even a failure's `None`) parks under the viewed key so a dead file isn't re-streamed every
    /// pass; and a live rescan re-keys everything by identity, so the answer follows its ride.
    #[test]
    fn ride_track_request_hands_out_the_id_until_answered() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let ride = |name: &str| crate::ride::RideSummary {
            name: heapless::String::try_from(name).unwrap(),
            start_time: 1_720_000_000,
            distance_m: 1_000,
            moving_time_s: 600,
            climb_m: 10,
            synced: false,
        };
        app.set_rides(&[ride("A"), ride("B")], &[7, 9]);

        assert_eq!(app.take_ride_track_request(), None, "no detail open — no request");

        app.activity.viewed_ride = Some(1); // the Rides press's entry side-effect
        assert_eq!(app.take_ride_track_request(), Some(9), "the viewed ride's durable id");
        assert_eq!(app.take_ride_track_request(), Some(9), "re-polls until the host answers");

        app.set_ride_profile(None); // a failed stream still answers — no per-pass grind
        assert_eq!(app.take_ride_track_request(), None, "answered for this ride");

        // A rescan drops ride A: id 9 moves to index 0. The viewed key and the answer key both
        // follow by identity, so nothing re-fires.
        app.set_rides(&[ride("B")], &[9]);
        assert_eq!(app.activity.viewed_ride, Some(0), "the viewed index follows the id");
        assert_eq!(app.take_ride_track_request(), None, "the answer moved with it");

        // The viewed ride itself vanishing clears the keys — nothing left to request.
        app.set_rides(&[ride("A")], &[7]);
        assert_eq!(app.activity.viewed_ride, None);
        assert_eq!(app.take_ride_track_request(), None);
    }
}
