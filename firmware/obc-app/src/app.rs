//! [`AppState`] — the device's view state — and [`App`], the shared per-frame
//! driver that both hosts run.

use embedded_graphics::{draw_target::DrawTarget, primitives::Rectangle};
use obc_elevation::ElevationSource;
use obc_reader::Reader;
use obc_render::{zoom_for_mpp, Canvas, Clock, NoopClock, RenderScratch, RenderStats, Viewport};
use obc_route::{Profile, RouteReader};

use crate::activity::{Activity, Mode};
use crate::card_scheduler::{BootUpdate, DfuLanding, PendingUpload, UploadEvent};
use crate::catalog_state::CatalogState;
use crate::device_core::core_mode::{CoreMode, ModeState};
use crate::device_core::storage_info::StorageInfo;
use crate::dfu::DfuState;
use crate::dirty::Dirty;
use crate::host::{DrainStatus, HostCommand, HostCommandClass, HostMailbox};
use crate::input::{Chord, Gesture};
use crate::navigator::PlanFamily;
use crate::navigator::{NavigatorIntent, NavigatorMachine, PlanPhase};
use crate::placement::define_placement_constructors;
use crate::ride::RideSummary;
use crate::ride_engine::RideEngine;
use crate::route::RouteSummary;
use crate::screen::{self, Ctx, MapScreen, QuickDrawerScreen, Render, RenderFrame, Screen, WarningFlags};
use crate::settings::{DateTime, Settings};
use crate::ui_runtime::UiRuntime;
use crate::wall_clock::WallClock;
use crate::DeviceStatus;
use obc_map_scene::MapScene;
use obc_ports::{Fix, InputClock, InputSource, LocationSource, RideClock, Sensors, TrackPoint};

/// How the camera relates to the user's position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    /// The camera tracks the user — every fix recenters the map on it. The normal navigation mode.
    Follow,
    /// The camera is driven manually (the simulator's mouse pan/zoom) and ignores the user's
    /// position; fixes are still recorded for the marker.
    Free,
}

/// What Up/Down moves along while [pan mode](Pan) is in [`Move`](PanTool::Move).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanBasis {
    /// Back / ahead on the active route's cumulative-distance axis.
    Route,
    /// Up / down in screen space.
    Vertical,
    /// Left / right in screen space.
    Horizontal,
}

impl PanBasis {
    /// The other screen-space Free axis. Route falls back to Vertical; callers normally use the
    /// remembered Free basis instead so Route can reach either axis intentionally.
    fn toggled_free(self) -> Self {
        match self {
            PanBasis::Vertical => PanBasis::Horizontal,
            PanBasis::Route | PanBasis::Horizontal => PanBasis::Vertical,
        }
    }

    /// Unit screen-space direction a **positive** step pans the camera centre toward. Route motion
    /// has no screen-space unit: it is resolved against the streamed route by [`AppState::sync_pan_route`].
    fn screen_unit(self) -> Option<(f32, f32)> {
        match self {
            PanBasis::Route => None,
            PanBasis::Vertical => Some((0.0, -1.0)),
            PanBasis::Horizontal => Some((1.0, 0.0)),
        }
    }
}

/// What Up/Down does in pan mode. A Select tap toggles this independently of [`PanBasis`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanTool {
    /// Move the camera along the selected route/free basis.
    Move,
    /// Change zoom while keeping the detached camera centre fixed.
    Zoom,
}

/// Active **pan-mode** state. While this is `Some`, the camera is detached
/// ([`Free`](CameraMode::Free)) and frozen where the rider left it: GPS fixes no
/// longer recenter it, and the map rotation is locked to
/// [`frozen_course_rad`](Pan::frozen_course_rad) so a live heading update can't spin
/// the map under the pan. `None` = the normal Follow map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pan {
    /// The route/free axis Up/Down moves along when [`tool`](Pan::tool) is [`Move`](PanTool::Move).
    pub basis: PanBasis,
    /// Whether Up/Down moves or zooms.
    pub tool: PanTool,
    /// The frozen map rotation (radians CW from north), snapshotted on entry so the map never
    /// rotates while it is being inspected.
    pub frozen_course_rad: f32,
    /// The inspection cursor on the route's cumulative-distance axis. Kept when moving freely so
    /// returning to Route resumes at the same inspected point rather than at the live rider.
    pub route_progress_m: u32,
    /// Free is a stable mode family, not two adjacent stops in a three-state ring. Remember its
    /// last axis while Route is active so Select-hold can return without changing direction.
    last_free_basis: PanBasis,
    /// A route step or basis change owes one cold `position_at` lookup at the pre-draw boundary.
    /// Private to the app: screens may inspect the mode, never acknowledge route I/O.
    route_camera_dirty: bool,
}

/// The device's view state: where the camera looks, how zoomed in it is, what mode it's in, and
/// the last known user fix. Small platform-fed facts shown by app chrome live together in
/// [`device`](AppState::device); weather, catalogs, navigation, and transfers keep their own state.
///
/// The shared core the host renders. The host owns the display size and the
/// [`obc_render::RenderScratch`]/draw target; each frame it calls [`update`] with the platform's
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
    /// freezes the rotation (see [`Pan`]); the Map screen binds the Select/Back to
    /// panning while it's set and draws the pan HUD over the map.
    pub pan: Option<Pan>,
    /// Latest electronic-compass heading (degrees CW from north), or `None` until one
    /// arrives. Stands in for the GPS course when the rider is stopped on a heading-up
    /// map, so the orientation follows the compass instead of snapping to north; only
    /// adopted on ticks where it would actually drive the rotation (see [`App::tick`]).
    pub compass_deg: Option<f32>,
    /// Small, current platform-fed facts rendered by ordinary app chrome.
    pub device: DeviceStatus,
    /// The Bluetooth screen's **"Forget phone"** request (epic #447, P8): set by the screen's
    /// guarded hold, drained by the host via the pass — which clears the RRAM bond
    /// slot and drops the bonded connection on the board, or clears the injected `paired` flag in
    /// the sim. A pending app→host command, carried here because `AppState` is the one mutable
    /// app-wide state a screen's `handle` reaches — the last request that still works this way.
    pub ble_forget_pending: bool,
    /// Whether the loaded map carries a non-empty §8 nav graph (#882) — fed once at map open by
    /// [`App::set_map_nav_graph`]; the Detour station/chooser gate on it (a graph-less map dims
    /// the station instead of failing a plan). Carried here because both `handle` (the gate) and
    /// `draw` (the dimming) need it without a `Reader`.
    pub has_nav_graph: bool,
    /// The rain map's selected **time step** (WX11): `0` = the current frame, `n` = the n-th
    /// future frame of the active bundle. Rider *selection* state, which is why it stays in the UI
    /// plane while the range it clamps against
    /// ([`WeatherDomain::steps_ahead`](crate::weather::WeatherDomain::steps_ahead)) does not.
    /// Written by the rain-map screen's Step arm and reset to `0` on every entry/exit, so it can
    /// never leak a stale offset; read by the host when it leases the frame's
    /// [`RainOverlayAdapter`](crate::RainOverlayAdapter) (`at_step`), so the leased raster and the
    /// on-screen frame timestamp are one decision.
    pub rain_step: u8,
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
            device: DeviceStatus {
                // Stand-in until a [`FuelGauge`](obc_ports::FuelGauge) feeds a real reading on the first tick.
                battery_pct: 75,
                // No phone linked until the host feeds the first [`BleStatus`](crate::BleStatus).
                ble_link: crate::BleLink::Advertising,
                ble_paired: false,
            },
            ble_forget_pending: false,
            has_nav_graph: false,
            rain_step: 0,
        }
    }

    /// Clamp the camera to the rain map's zoom-out `floor` — the smallest zoom at which the active
    /// product's raster still renders, derived by
    /// [`WeatherDomain`](crate::weather::WeatherDomain) and passed in by the caller that has it.
    /// Called by the weather screens on rain-map entry and after each Inspect zoom step, and by the
    /// pass's own tail while the rain map is up; a disengaged floor (`0.0`) is a no-op, and zooming
    /// *in* is never touched.
    pub fn clamp_rain_zoom(&mut self, floor: f32) {
        if floor > 0.0 && self.zoom < floor {
            self.zoom = floor;
        }
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
            Some(pan) => pan.frozen_course_rad,
            None if self.heading_up => self.live_course_rad(),
            None => 0.0,
        }
    }

    /// The heading-up angle to freeze from the latest fix right now: the GPS course, or the
    /// electronic compass when stopped (no course), or 0 (north) when neither is known. Used by
    /// [`course_rad`](AppState::course_rad) and snapshotted once on entering Inspect.
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

    /// Enter **pan mode**: detach the camera ([`Free`](CameraMode::Free)) so fixes stop recentering
    /// it, snapshot the current orientation, and start in Move. A loaded route makes route-relative
    /// movement the default; a route-less browse starts on the vertical free axis.
    pub fn enter_pan(&mut self, has_route: bool, route_progress_m: u32) {
        self.mode = CameraMode::Free;
        self.pan = Some(Pan {
            basis: if has_route { PanBasis::Route } else { PanBasis::Vertical },
            tool: PanTool::Move,
            frozen_course_rad: self.live_course_rad(),
            route_progress_m,
            last_free_basis: PanBasis::Vertical,
            route_camera_dirty: has_route,
        });
    }

    /// Leave pan mode: drop the pan state, resume [`Follow`](CameraMode::Follow), and
    /// recenter on the last fix so the rider snaps straight back onto themselves.
    pub fn exit_pan(&mut self) {
        self.pan = None;
        self.mode = CameraMode::Follow;
        self.recenter_on_user();
    }

    /// Recenter the camera on the last known fix. Pan mode has no standalone recenter action;
    /// leaving it returns to Follow and calls this helper. No-op before the first fix.
    pub fn recenter_on_user(&mut self) {
        if let Some(fix) = self.user_fix {
            self.cam_lon = fix.lon;
            self.cam_lat = fix.lat;
        }
    }

    /// Toggle Move ↔ Zoom (Select tap). No-op when not panning.
    pub fn toggle_pan_tool(&mut self) {
        if let Some(pan) = self.pan.as_mut() {
            pan.tool = match pan.tool {
                PanTool::Move => PanTool::Zoom,
                PanTool::Zoom => PanTool::Move,
            };
        }
    }

    /// Toggle the movement **family** Route ↔ Free (Back hold). The last Free axis is restored, so
    /// changing families never also changes axis. Back hold is authoritative even from Zoom: the
    /// destination always opens in Move, avoiding a dead-feeling hold and a second gesture. With no
    /// active route it can only leave Zoom for Free Move; in Free Move it remains a no-op.
    pub fn toggle_pan_family(&mut self, has_route: bool) {
        if let Some(pan) = self.pan.as_mut() {
            if !has_route {
                pan.tool = PanTool::Move;
                return;
            }
            if pan.basis == PanBasis::Route {
                pan.basis = pan.last_free_basis;
            } else {
                pan.last_free_basis = pan.basis;
                pan.basis = PanBasis::Route;
                pan.route_camera_dirty = true;
            }
            pan.tool = PanTool::Move;
        }
    }

    /// Toggle Free Vertical ↔ Free Horizontal (Select hold). No-op in Route and Zoom: this gesture
    /// changes only an already-active Free axis, never the movement family or tool.
    pub fn toggle_pan_free_axis(&mut self) {
        if let Some(pan) = self.pan.as_mut() {
            if pan.tool == PanTool::Zoom || pan.basis == PanBasis::Route {
                return;
            }
            pan.basis = pan.basis.toggled_free();
            pan.last_free_basis = pan.basis;
        }
    }

    /// Apply `steps` from Up/Down to the active pan tool. Zoom uses the normal map's fixed 1.2×
    /// steps. Free movement travels [`PAN_STEP_PX`] screen pixels. Route movement travels the same
    /// visual distance converted to ground metres, then defers its one geometry lookup to
    /// [`sync_pan_route`](Self::sync_pan_route).
    pub fn pan_step(&mut self, steps: i32, route_total_m: u32) {
        let Some(pan) = self.pan else { return };
        if pan.tool == PanTool::Zoom {
            self.zoom = step_zoom(self.zoom, steps, MIN_ZOOM, MAX_ZOOM);
            return;
        }
        if pan.basis == PanBasis::Route {
            let vp = Viewport::new_rotated(0.0, 0.0, self.cam_lon, self.cam_lat, self.zoom, self.course_rad());
            let metres_per_step = (vp.meters_per_pixel() * PAN_STEP_PX + 0.5).max(1.0) as i64;
            let next =
                (pan.route_progress_m as i64 + steps as i64 * metres_per_step).clamp(0, route_total_m as i64) as u32;
            if let Some(pan) = self.pan.as_mut() {
                pan.route_camera_dirty |= next != pan.route_progress_m;
                pan.route_progress_m = next;
            }
            return;
        }
        if let Some((ux, uy)) = pan.basis.screen_unit() {
            let d = steps as f32 * PAN_STEP_PX;
            self.pan_by_pixels(ux * d, uy * d);
        }
    }

    /// Resolve a dirty route inspection cursor to its coordinate. Called once at the App's
    /// pre-draw boundary, where the active [`RouteReader`] exists; gesture handling deliberately
    /// owns no reader and drawing remains read-only.
    fn sync_pan_route(&mut self, route: &RouteReader) {
        let Some(pan) = self.pan else { return };
        if pan.basis != PanBasis::Route || !pan.route_camera_dirty {
            return;
        }
        let progress_m = pan.route_progress_m.min(route.total_distance_m);
        let position = route.position_at(progress_m);
        if let Some(pan) = self.pan.as_mut() {
            pan.route_progress_m = progress_m;
            pan.route_camera_dirty = false;
        }
        if let Some(position) = position {
            self.cam_lon = position.lon;
            self.cam_lat = position.lat;
        }
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

/// Camera travel **per Up/Down step** in pan mode, in screen pixels — a *screen* amount (not
/// ground metres), so panning is finer when zoomed in.
pub const PAN_STEP_PX: f32 = 40.0;

/// Zoom multiplier per Up/Down step, shared by the Follow map and pan mode's Zoom tool.
pub(crate) const ZOOM_STEP: f32 = 1.2;
/// Zoom clamps (pixels per microdegree-lat), shared by both map modes.
pub(crate) const MIN_ZOOM: f32 = 1e-6;
pub(crate) const MAX_ZOOM: f32 = 1e4;

/// Apply the app's signed multiplicative zoom step within a caller-owned range.
pub(crate) fn step_zoom(mut zoom: f32, steps: i32, min: f32, max: f32) -> f32 {
    let step = if steps >= 0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
    for _ in 0..steps.unsigned_abs() {
        zoom *= step
    }
    zoom.clamp(min, max)
}

/// Capacity of one frame's gesture buffer ([`App::handle_input`], [`App::recognize`]). One frame
/// yields at most one gesture per raw event (the input queue is bounded — `ButtonInput`'s is 8)
/// plus the single per-frame long-press, so this never overflows.
pub const GESTURE_BUF: usize = 16;

/// The whole device application, ready to run a frame.
///
/// The single entry point both hosts share: each constructs one `App`, then per frame
/// [`tick`](App::tick)s it with their [`LocationSource`], feeds raw controls through
/// [`handle_input`](App::handle_input), and [`render_frame`](App::render_frame)s to their display.
/// `App` owns the screen stack, the input + overlay plane ([`InputPlane`]), the camera
/// [`AppState`] and the ride [`Activity`]. It does **not** own the render path's scratch: the host
/// keeps a [`RenderScratch`] and lends it to each render call (#1146).
///
/// The firmware can split the two planes across executors — recognising gestures on a
/// high-priority [`InputPlane`] that preempts the map render and feeding them back through
/// [`apply_gesture`](App::apply_gesture); [`handle_input`](App::handle_input) is those halves fused
/// for the single-loop hosts.
///
/// ```ignore
/// let mut app = App::new(AppState::new(cx, cy, zoom));
/// let mut scratch = RenderScratch::new(); // the host's, lent per frame
/// loop {
///     // GPS + barometer + compass + active route → camera, map-match, ride stats.
///     // Only the capabilities this host has; `Sensors::new` leaves the rest (here: the BLE
///     // strap's heart rate / power / cadence) absent.
///     let sensors = Sensors {
///         altimeter: Some(&mut baro),
///         temperature: Some(&mut thermometer),
///         clock: Some(&mut gps_clock),
///         compass: Some(&mut compass),
///         track: Some(&mut track_log),
///         fuel: Some(&mut fuel_gauge),
///         ..Sensors::new(&mut location_source)
///     };
///     app.tick(RideClock(now_ms), sensors, route.as_ref());
///     app.handle_input(InputClock(now_ms), &mut input_source); // Select + Back → gestures
///     app.render_frame(Some(&mut scratch), &mut display, &reader, route.as_ref(), w, h, color_policy);
/// }
/// ```
/// Whether — and by which source — the wall clock has been established from a **real time source
/// this boot**. The safety core of the auto-expiry epic (#638): the device has no RTC, so at boot
/// the clock resumes from a persisted set-point that is stale by the powered-off span. That stale
/// clock is [`Untrusted`](ClockTrust::Untrusted); it advances to [`Gps`](ClockTrust::Gps) or
/// [`Ble`](ClockTrust::Ble) **only** when that source stamps the clock this boot (via
/// [`App::stamp_clock`]). The expiry sweep (S3) refuses to stamp or delete anything while untrusted,
/// so a stale or fat-fingered clock can never drive a deletion. **Never persisted** — every power
/// cycle resets it to `Untrusted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockTrust {
    /// No real time source has stamped the clock this boot: it is the stale persisted set-point (or
    /// the factory default). Display-only — no stamps, no deletions.
    Untrusted,
    /// A GPS fix stamped the clock this boot (the fix payload carries full UTC date + time).
    Gps,
    /// A BLE `setClock` from the phone stamped the clock this boot (epic #638 S2, #642).
    Ble,
}

/// How stale the last GPS fix may be (map-plane ms) and still serve in the weather request context
/// as "where the rider is" (WX8, #1193): a 30-second-old fix is still the rider to within metres,
/// while a tunnel or indoor stop past that reads as *no position*. The request still raises for
/// diagnostics/retry, but today's companion cannot fetch until a fresh device fix arrives.
pub const WEATHER_FIX_FRESH_MS: u32 = 30_000;

pub struct App {
    /// The camera / orientation / last-fix state — public so the host's mouse pan/zoom and control
    /// panel can read and adjust it directly.
    pub state: AppState,
    /// The ride mode + tracking accumulators.
    pub activity: Activity,
    /// The resident route / ride / trip catalogs keyed by durable object ids, plus the
    /// identity-keyed view caches (ride profile/preview, nav preview) — the one component owning
    /// the id ↔ summary pairing and every rescan-remap invariant (#450, epic #526). Populated by
    /// the host through the `set_*` façade methods below.
    pub(crate) catalogs: CatalogState,
    /// The loaded map's routing-profile **names** (routing-v2 N5), refreshed by the host on map load
    /// ([`set_nav_profiles`](App::set_nav_profiles)) — resident because the Bike-type settings screen
    /// and the created-route overview label render them on frames the host draws without a `Reader`.
    /// Only the names are mirrored (≤ 8 × 12 B); the multiplier tables stay solely in `MapTables`.
    nav_profiles: crate::NavProfiles,
    /// The ride-domain component: the live route-matcher, the once-per-load route caches
    /// (elevation profile, climbs, waypoints) with their build keys, the resident climb detail
    /// buffer, and the tick-edge state (fix freshness, sensor-tile
    /// edges, battery-poll cadence, ambient temperature). `App::tick` orchestrates it each frame.
    pub(crate) ride: RideEngine,
    /// The UI-plane component: the screen stack, the fused input plane, the map-plane clock,
    /// repaint accumulation (full-frame + region) and wake scheduling, hold cancellation, the
    /// idle-return policy, and the [`CardScheduler`](crate::card_scheduler::CardScheduler) that
    /// owns every host-pushed card. `pub(crate)` so the in-crate harnesses and the scheduler's own
    /// tests can observe the stack they act on; no public accessor exists for it.
    pub(crate) ui: UiRuntime,
    /// The persisted device settings, seeded from the host's store at boot
    /// ([`set_settings`](App::set_settings)) and edited in place by the settings screens.
    settings: Settings,
    /// The live wall clock: [`settings.clock`](Settings::clock) (a set-point) advanced by elapsed
    /// monotonic millis — there's no RTC, so this is how a static readout ticks. Re-stamped whenever
    /// the set-point changes in [`set_settings`](App::set_settings) /
    /// [`apply_gesture`](App::apply_gesture). See [`WallClock`].
    wall_clock: WallClock,
    /// Whether the wall clock has been established from a real time source **this boot** (see
    /// [`ClockTrust`]). Starts [`Untrusted`](ClockTrust::Untrusted) at every boot — the persisted
    /// set-point is display-only — and only [`stamp_clock`](App::stamp_clock) (GPS now, BLE in S2)
    /// advances it. Read through [`clock_trusted`](App::clock_trusted); the auto-expiry sweep (#638
    /// S3) gates every stamp and deletion on it. **Never persisted.**
    clock_trust: ClockTrust,
    /// The retention domain (epic #638 S3, #1437): the whole auto-expiry policy — the trusted-clock
    /// and hourly gates, the usage and sync stamps, expiry discovery, live revalidation, and the
    /// delete retry pacing. Advanced from [`tick`](App::tick); it emits typed metadata effects and
    /// catalog expiry intents, which the compatibility seam below still translates into the legacy
    /// [`HostCommand`] protocol.
    pub(crate) retention: crate::retention::RetentionMachine,
    /// The **Recorder** domain (#1398 R1/R2): the ride session identity, whether a ride is open,
    /// the rider's undelivered close, the checkpoint deadline, the boot-recovery decision, and the
    /// two per-session buffers a new ride restarts. The only thing in the app that decides a ride
    /// is open or closed.
    pub recorder: crate::recorder::RecorderMachine,
    /// The weather domain (#1437): the installed data's identity and revision, visible freshness,
    /// the refresh request and its in-flight operation, the last terminal result, and the alert
    /// decision. The bundle itself stays in the platform's store — this owns what the rider is
    /// *told*, never the frames.
    pub(crate) weather: crate::weather::WeatherDomain,
    /// The **Navigator** domain (#1397 S2): the rider's undelivered plan requests, the per-family
    /// phase, and the operation token every planner answer must carry back. The only writer of any
    /// of them, and of [`mode`](App::mode)'s two search levels.
    pub(crate) navigator: NavigatorMachine,
    /// **`CoreMode`** (#1397 S5): the one owner of "what heavy work may run now, and what the rider
    /// is looking at" — the two search levels Navigator writes, the transfer level
    /// [`set_map_transfer`](App::set_map_transfer) writes, and the Recalculating banner's
    /// level→edge bit. Every reader of "a search is live" derives from it; nothing keeps a second
    /// copy.
    pub(crate) mode: CoreMode,
    /// The **settings-persistence** machine (#810, #1397 S2): the dirty revision, the subtree
    /// debounce, the retry backoff and the stale-answer rule.
    pub(crate) settings_ops: crate::settings::SettingsMachine,
    /// The same machine again (#1542), for the **alert-marks record**. A second record needs a
    /// second *instance*, not a second policy: it inherits the revision guard, the backoff and the
    /// stale-ack rule verbatim, and differs only in what stage 9 hands it — a storm is not a rider
    /// edit, so its write is never subtree-gated.
    pub(crate) alert_marks_ops: crate::settings::SettingsMachine,
    /// The **DFU** domain (#1397 S2): the single most-recent-wins update phase and its token.
    pub(crate) dfu: DfuState,
    /// The **StorageInfo** domain (#1397 S2): the free-space refresh, its token, and the figure the
    /// System screen prints.
    pub(crate) storage: StorageInfo,
    /// The DeviceCore coordinator's own state (#1438): every cross-domain connection, the levels a
    /// stage detects an edge against, the current [`Capabilities`](crate::device_core::Capabilities)
    /// and the re-entrancy guard. Not domain state — nothing here decides a product rule.
    pub(crate) pass: crate::device_core::pass::PassState,
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
    /// Whether this platform's panel has a **controllable light** —
    /// [`Backlight::available`](obc_ports::Backlight)'s answer, declared once by the host at
    /// composition through [`set_backlight_available`](App::set_backlight_available). A constant
    /// of the hardware, not a preference, which is why it is not a settings row.
    ///
    /// `false` removes the quick drawer's brightness control altogether. Defaults to `false`, so a
    /// platform that says nothing does not offer a control it has no port for.
    backlight_available: bool,
}

/// Where a boot seed of the weather alert-mark anchors came from — the one thing
/// [`App::set_alert_marks`] cannot work out for itself, and the whole of what decides whether the
/// seed still owes a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarksProvenance {
    /// The marks record answered (or held nothing) — already persisted.
    Record,
    /// The one-time fallback read the anchors out of a stored v16 preferences blob's frozen span.
    /// The update must not cost the rider their anchors, so they are rehomed into the record.
    LegacyBlob,
}

/// Cap on the computed route's shape-preview polyline (#685 §4): the host decimates the planned
/// polyline to at most this many points before handing it to
/// [`set_nav_preview`](App::set_nav_preview) — plenty for the overview's ~212×90 px sketch, and a
/// fixed ~512 B resident buffer here rather than a route-sized one.
pub const NAV_PREVIEW_MAX: usize = 64;

impl App {
    /// Build the app straight onto the live map: stack `[Home, Map]`, Home the always-present root
    /// that Finish / Discard return to, no route loaded. The map-first constructor the simulator
    /// uses for headless `--png` renders (and the tests); the GUI and device boot via
    /// [`new_idle`](App::new_idle).
    pub fn new(state: AppState) -> Self {
        let mut app = Self::new_idle(state);
        app.open_map_first();
        app
    }

    /// The map-first tail both map-first constructors share: drop the just-built idle app straight
    /// onto the live Map. Plain safe mutation of a complete `App` (the assignment drops the Idle
    /// activity it replaces).
    fn open_map_first(&mut self) {
        self.activity = Activity::new(Mode::Riding);
        let _ = self.ui.stack.push(Screen::Map(MapScreen::new()));
    }

    define_placement_constructors!(
        /// Build the app at the device's real power-on state: the Home screensaver, Idle, no route
        /// loaded. Loading a route (Home → Menu → Routes → `press`) starts riding and opens the Map.
        pub fn new_idle(state: AppState);
        /// Build the idle power-on [`App`] **in place** at `slot` — the by-reference twin of
        /// [`new_idle`](App::new_idle), used by firmware to construct the resident `App` without
        /// materializing it on the stack. Each KB-scale component is written by its own placement
        /// constructor. The render scratch is not part of `App` and remains host-owned.
        pub unsafe fn init_idle;
        fields {
            state: state,
            activity: Activity::new(Mode::Idle),
            catalogs: CatalogState::new() => CatalogState::init_in_place,
            ride: RideEngine::new() => RideEngine::init_in_place,
            ui: UiRuntime::new() => UiRuntime::init_in_place,
            nav_profiles: crate::NavProfiles::new(),
            settings: Settings::default(),
            // The clock starts from the default set-point; the host re-stamps the persisted value.
            wall_clock: WallClock::new(Settings::default().local_clock()),
            // A persisted set-point is display-only until GPS or BLE establishes trust this boot.
            clock_trust: ClockTrust::Untrusted,
            retention: crate::retention::RetentionMachine::new(),
            recorder: crate::recorder::RecorderMachine::new() => crate::recorder::RecorderMachine::init_in_place,
            weather: crate::weather::WeatherDomain::new(),
            navigator: NavigatorMachine::new(),
            mode: CoreMode::new(),
            settings_ops: crate::settings::SettingsMachine::new(),
            alert_marks_ops: crate::settings::SettingsMachine::new(),
            dfu: DfuState::new(),
            storage: StorageInfo::new(),
            pass: crate::device_core::pass::PassState::new(),
            fw_version: heapless::String::new(),
            map_name: heapless::String::new(),
            map_obcm_version: 0,
            backlight_available: false,
        }
    );

    /// Build the **map-first** [`App`] in place at `slot` — the by-reference twin of
    /// [`new`](App::new), as [`init_idle`](App::init_idle) is the twin of
    /// [`new_idle`](App::new_idle). Initialises the idle state, then drops straight onto the live
    /// Map (stack `[Home, Map]`, Riding) — the placement path a firmware bring-up uses to put the
    /// map on glass before buttons exist.
    ///
    /// # Safety
    /// Same contract as [`init_idle`](App::init_idle).
    pub unsafe fn init_map(slot: *mut App, state: AppState) {
        // SAFETY: caller's contract. `init_idle` fully initialises the slot, so `&mut *slot` is
        // sound thereafter.
        unsafe { Self::init_idle(slot, state) };
        unsafe { &mut *slot }.open_map_first();
    }

    /// Assert the [`new_idle`](App::new_idle) boot state, field by field, delegating each KB-scale
    /// component to its own boot-state assertion. The destructure is exhaustive, so a field added
    /// to the plan must state its boot value here too.
    #[cfg(test)]
    fn assert_idle_boot_state(&self, state: AppState) {
        use crate::retention::SweepKind;
        let App {
            state: camera,
            activity,
            catalogs,
            ride,
            ui,
            nav_profiles,
            settings,
            wall_clock,
            clock_trust,
            retention,
            recorder,
            weather,
            navigator,
            mode,
            settings_ops,
            alert_marks_ops,
            dfu,
            storage,
            pass,
            fw_version,
            map_name,
            map_obcm_version,
            backlight_available,
        } = self;
        assert_eq!(*camera, state, "the camera state is preserved verbatim");
        assert_eq!(activity.mode, Mode::Idle, "boots Idle, not Riding");
        assert!(activity.active_route.is_none() && activity.active_climb.is_none(), "nothing loaded, no climb");
        assert!(activity.next_waypoint.is_none(), "no next waypoint at power-on");
        catalogs.assert_boot_state();
        ride.assert_boot_state();
        ui.assert_boot_state();
        assert!(nav_profiles.is_empty(), "no routing profiles before a map loads");
        assert_eq!(*settings, Settings::default(), "the defaults until the store answers");
        assert_eq!(*wall_clock, WallClock::new(Settings::default().local_clock()), "the default set-point");
        assert_eq!(*clock_trust, ClockTrust::Untrusted, "a persisted set-point is display-only this boot");
        assert!(
            [SweepKind::DeleteRoute, SweepKind::StampRoute, SweepKind::DeleteRide, SweepKind::StampRide]
                .iter()
                .all(|k| !retention.has(*k)),
            "no retention sweep in flight"
        );
        assert!(
            weather.installed().is_none() && !weather.refreshing() && weather.last_refresh().is_none(),
            "no weather installed, none requested, nothing completed this boot"
        );
        assert_eq!(weather.alert_marks(), &[None; crate::weather_alerts::ALERT_CLASSES], "no alert anchors at boot");
        assert!(settings_ops.is_empty(), "settings Clean at revision 0");
        assert!(alert_marks_ops.is_empty(), "the marks record Clean at revision 0");
        navigator.assert_boot_state();
        recorder.assert_boot_state();
        assert_eq!(*mode, CoreMode::new(), "nothing searching, nothing streaming, no banner shown");
        dfu.assert_boot_state();
        storage.assert_boot_state();
        assert_eq!(*pass, crate::device_core::pass::PassState::new(), "no connection wired, no pass in flight");
        assert!(fw_version.is_empty() && map_name.is_empty(), "the host has identified nothing yet");
        assert_eq!(*map_obcm_version, 0, "no map format known yet");
        assert!(!*backlight_available, "no host has claimed a panel light yet");
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
    ///
    /// Two things happen, and the DeviceCore pass runs them at different stages: the world is
    /// applied ([`advance_inputs`](App::advance_inputs), stage 3) and then the retention domain
    /// advances ([`retention_tick`](App::retention_tick), stage 5). One implementation of each,
    /// reached by both compositions.
    pub fn tick(&mut self, clock: RideClock, sensors: Sensors, route: Option<&RouteReader>) {
        self.advance_inputs(clock, sensors, route);
        // Auto-expiry (epic #638, S3): stamp the active route's `last_used` on activation, then run
        // the roughly-hourly sweep — both gated on a trusted clock and no ride recording. Deletes +
        // stamps leave here as typed host commands.
        self.retention_tick();
    }

    /// Apply the world to the app: the sensor ports, the fix and its derived readouts, and the
    /// repaint edges they imply. The sensor half of [`tick`](App::tick), and the DeviceCore pass's
    /// third stage.
    pub(crate) fn advance_inputs(&mut self, clock: RideClock, sensors: Sensors, route: Option<&RouteReader>) {
        let now_ms = clock.0;
        // BLE-sensor freshness is judged on the `RideClock` (`now_ms`) — the clock ride samples and
        // summaries use. Remember it so the stat tiles, which render *after*
        // this tick against the map-plane clock `self.ui.now_ms`, judge staleness on the same timebase.
        // On the board `self.ui.now_ms == now_ms` (the ride loop drives `advance_animations` and `tick`
        // off one monotonic `now`); in the simulator they differ (`RideClock` is GPX-playback time,
        // `self.ui.now_ms` is wall time), and a tile reading `self.ui.now_ms` would blank to `--` seconds
        // into a replay — see the `sensor_tiles_…` test.
        self.activity.note_sensor_clock(now_ms);
        // Read the freeze once, before anything below can move the stack: while a planner run holds
        // the arena over a map base, this tick must not advance route-match progress (see the fix
        // path below for why, and `device_core::core_mode` for the whole rule).
        let frozen = self.reroute_freeze_active();
        // The once-per-load route/session sync — matcher re-lock, session restart (accumulators +
        // breadcrumb), the route-length mirror, and the climbs/waypoints cache builds — is the
        // ride engine's; a change there (route line appeared/vanished, breadcrumb cleared)
        // repaints the map even on a frame with no fresh fix.
        if self.ride.sync_route_state(&mut self.activity, route) {
            self.ui.map_dirty = true;
        }
        // A detour commit queues a seam re-anchor because the commit handler owns no host
        // `RouteReader`. Install matcher progress + the forward-only floor at the splice seam
        // before this tick's fresh fix, then re-derive every guidance consumer from it.
        if self.ride.apply_pending_seam(&mut self.activity, route) {
            if let Some(route) = route {
                self.update_active_climb(route);
                self.update_next_waypoint(route);
            }
            self.ui.map_dirty = true;
        }

        let Sensors { loc, altimeter, temperature, clock, compass, track, fuel, hr, power, cadence } = sensors;
        // Battery charge from the PMIC gauge, on the slow ~30 s cadence. Nothing here says
        // "repaint": the gauge is drawn by Home alone, Home's row declares
        // [`RenderKeyKind::Home`](crate::screen::RenderKeyKind) and that key carries the level — so
        // a change repaints exactly when Home is visible, and the riding views that never draw it
        // are not woken for a full ~97 ms map render every 30 s.
        if self.ride.battery_poll_due(now_ms) {
            if let Some(soc) = fuel.and_then(|f| f.poll()) {
                self.state.device.battery_pct = soc;
            }
        }
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
                self.ride.temp_c = Some(c);
            }
        }
        // BLE sensors → the live values Activity staleness-gates + the per-ride summaries. Drained
        // here beside the altimeter/temperature so `record_motion` (below, on a fresh fix) sees this
        // tick's samples. `Some` only on a fresh reading; a dropped strap simply stops reporting and
        // the staleness gate expires the last value. The stat tiles (SE5) read these through the
        // `live_*_display` accessors, and the Statistics grid's render key names those same values,
        // so a fresh sample repaints the grid — and only the grid.
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
        // GPS UTC time → the wall clock. GPS **always** stamps now (manual date/time was removed in
        // #641, so a fat-fingered clock can't feed the expiry sweep). The receiver resolves time
        // before a 3D position, so this lands during acquisition — the clock can be right while the
        // "No GPS Fix" banner is still up. Funnels through `stamp_clock`, the one entry point that
        // owns the trusted-clock invariant (BLE `setClock` joins it in epic #638 S2).
        if let Some(t) = clock.and_then(|c| c.poll()) {
            // GPS carries no timezone — pass `None` to leave the persisted offset untouched (BLE
            // `setClock` is the only source that sets it).
            self.stamp_clock(t.utc, t.second, None, ClockTrust::Gps);
        }
        // GPS fix → camera + map-match + ridden distance/time (only on a fresh fix, so a dropout
        // doesn't re-run the matcher or double-count). A *logged* fix also feeds the breadcrumb +
        // ride log.
        if let Some(fix) = self.state.update(loc) {
            // Stamp the fix-freshness clock against `self.ui.now_ms` — the map-plane clock the banner's
            // staleness check + render read with. Off `AppState`, so a stationary fix that moves
            // nothing doesn't force a redraw here.
            self.ride.last_fix_ms = Some(self.ui.now_ms);
            // Arm the map-referenced altimeter's one terrain read for this fix (EL8, epic #1068).
            // Nothing is sampled here — `tick` holds no elevation source, and an SD tile read does
            // not belong in the middle of the fix path anyway. The host drains it right after this
            // tick through `sample_terrain`, at the fix cadence and never per frame.
            self.ride.pending_terrain = Some((fix.lat, fix.lon));
            if let Some(route) = route {
                // The **Recalculating freeze** (issue #1146, P2) pauses exactly this: the matcher.
                // The frozen frame on glass shows the progress of the fix it was drawn from, and a
                // search can replace the geometry that progress is measured along — so advancing it
                // under a map nobody is redrawing is drift the rider cannot see. Everything else
                // this tick keeps running (the fix is recorded, the breadcrumb grows, the ride
                // totals and the altimeter accumulate): a freeze pauses the map, not the ride. The
                // two derived readouts below re-run against the *held* progress, so they are
                // idempotent while frozen and re-lock from the fresh match the moment it lifts.
                if frozen {
                    // The cursor stands still while the fixes keep coming, so the next match must
                    // not be judged against a one-fix-wide forward window: arm the wide re-lock.
                    self.ride.note_unmatched_fix();
                } else {
                    self.ride.match_fix(&mut self.activity, fix, route);
                }
                // "Am I on a climb now?" is derived from the fresh match — with hysteresis, and a
                // detail-profile refill only on a new climb entry (see `update_active_climb`).
                self.update_active_climb(route);
                // "Which waypoint is next?" from the same fresh progress — distance-lingered, and it
                // re-windows a truncated table forward as the rider advances (see below).
                self.update_next_waypoint(route);
            }
            // The WX12 ride-weather inputs, from the same fresh fix: the recent moving-speed
            // window (the projection's pace) and the travel direction (the wind arrows' frame of
            // reference — the route's general heading at the fresh match, else neutral). A freeze
            // holds the previous direction like it holds the matcher: the progress on glass hasn't
            // moved, so neither has the heading derived from it.
            if let Some(speed) = fix.speed_mps {
                self.recorder.speed_win.push_mps(speed);
            }
            if !frozen {
                self.ride.update_travel(&self.activity, route);
            }
            let motion = self.activity.record_motion(fix, now_ms);
            if motion.log {
                self.recorder.breadcrumb.push(fix.lon, fix.lat);
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
                    // `on_warning` latches it once per boot, so a whole ride of failing writes
                    // raises one dismissable card, not a per-fix nag.
                    if logged.is_err() {
                        self.on_warning(WarningFlags::REC_ERROR);
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
        // **Nothing below this line asks for a repaint, and that is the point (#1447).** Three
        // edges used to be surfaced here by hand, each against a private mirror of the value it was
        // watching: the camera / marker / heading a fresh fix moved, the "No GPS Fix" banner
        // flipping on a *timer* rather than on a state change, and a live sensor tile's displayed
        // value going fresh or stale. All three are facts the riding views *declare* they draw, so
        // the pass compares them for every visible screen at its own boundary — and only for the
        // screens that draw them, which is the economy the hand-written base-screen gate and the
        // per-quantity guards existed to buy.
    }

    /// Give the **map-referenced altimeter** (EL8, epic #1068) its one terrain read for the latest
    /// GPS fix. Call it once per host pass, immediately after [`tick`](App::tick).
    ///
    /// Returns whether a sample was actually taken — `false` on any pass with no fresh fix, which
    /// is most of them. That one-shot is why this is safe to call every frame: `tick` arms the
    /// request on a fresh fix only, so the read happens **at the fix cadence**, never per frame. On
    /// device that matters concretely — a terrain sample is a 512 B tile, usually already in the
    /// four-slot cache and otherwise an SD read, which has no business on the render path.
    ///
    /// `elev` is the same [`ElevationSource`] the route emitter fills from: the mounted `.obcd`
    /// terrain, or [`NullElevation`](obc_elevation::NullElevation) where there is none. With the
    /// null source (or outside the raster's coverage) the sample is `None`, nothing is fed, the
    /// estimator never settles, and the Elevation tile keeps its pre-epic barometric reading.
    ///
    /// It is deliberately **not** part of [`tick`](App::tick): `Sensors` is `obc-ports` vocabulary
    /// and terrain is not a sensor — it is the map, which the app already reaches through its own
    /// seam. Keeping it here also keeps the source's `&mut` out of the fix path, where the board
    /// holds it as a `.bss` `&'static mut` shared with the planner.
    pub fn sample_terrain(&mut self, elev: &mut dyn ElevationSource) -> bool {
        let Some((lat, lon)) = self.ride.pending_terrain.take() else { return false };
        if let Some(map_m) = elev.sample(lat, lon) {
            self.activity.record_map_elevation(map_m);
        }
        true
    }

    /// Advance [`RetentionMachine`](crate::retention::RetentionMachine) one pass (epic #638 S3,
    /// #1437). The whole policy — the trusted-clock and recording gates included — lives in the
    /// domain; all this does is assemble the [`RetentionView`](crate::retention::RetentionView) of
    /// the catalogs, the clocks and the live gates that the domain reads.
    pub(crate) fn retention_tick(&mut self) {
        self.with_retention(|retention, view| retention.advance(view));
    }

    /// Run `f` against the retention domain and the read-only [`RetentionView`] of the rest of the
    /// app it decides from — disjoint field borrows, so the machine mutates its own queue while it
    /// reads the catalogs.
    ///
    /// The view is assembled fresh at every call site (the tick *and* each drain), so discovery and
    /// the just-in-time recheck can never read two different pictures.
    ///
    /// [`RetentionView`]: crate::retention::RetentionView
    pub(crate) fn with_retention<T>(
        &mut self,
        f: impl FnOnce(&mut crate::retention::RetentionMachine, &crate::retention::RetentionView) -> T,
    ) -> T {
        // A `None` clock is invariant 1: no real time source established it this boot, so the
        // domain stamps nothing, deletes nothing and sweeps nothing.
        let now_utc = self.clock_trusted().then(|| self.wall_unix_now());
        let now_ms = self.ui.now_ms;
        let recording = self.recorder.recording();
        let App { retention, catalogs, activity, settings, .. } = self;
        let view = crate::retention::RetentionView {
            now_utc,
            now_ms,
            recording,
            route_ids: catalogs.route_ids(),
            route_metas: catalogs.route_metas(),
            active_route: activity.active_route,
            ride_records: catalogs.ride_records(),
            ride_retention: settings.ride_retention,
        };
        f(retention, &view)
    }

    /// Force the auto-expiry sweep to run on the next eligible tick, ignoring the hourly gate (epic
    /// #638, S3) — a **test and simulator seam** with no production caller. The simulator's "+1 day"
    /// control uses it so a fast-forwarded clock sweeps immediately instead of waiting for the
    /// wall-clock hour to roll.
    ///
    /// The production path to the same fact is
    /// [`note_catalog_changed`](crate::retention::RetentionMachine::note_catalog_changed), which
    /// stage 5 calls when the catalog's identity set moves.
    pub fn force_retention_sweep(&mut self) {
        self.retention.note_catalog_changed();
    }

    /// Recompute [`Activity::active_climb`] from the freshly-matched `progress_m` — the ride
    /// engine's hysteresis + once-per-entry detail refill
    /// ([`RideEngine::update_active_climb`]) — then apply the App-plane consequences of a
    /// transition: one repaint, and the C5 host auto-switch off the same edge.
    fn update_active_climb(&mut self, route: &RouteReader) {
        if let Some((prev, next)) = self.ride.update_active_climb(&mut self.activity, route) {
            // No repaint request: the active climb is in the Statistics and Climb render keys, so
            // the riding views' climb-scoped readouts repaint from the declaration.
            // Host-driven auto-switch / auto-return (C5), off the same entry/exit edge.
            self.apply_climb_auto_switch(prev, next);
        }
    }

    /// Recompute [`Activity::next_waypoint`] from the freshly-matched `progress_m` — the ride
    /// engine's linger hysteresis + truncated-table re-window
    /// ([`RideEngine::update_next_waypoint`]) — repainting once when the next waypoint moved.
    fn update_next_waypoint(&mut self, route: &RouteReader) {
        // No repaint request: the next waypoint is in the Map and Statistics render keys, so the
        // chip and the fields repaint from the declaration.
        self.ride.update_next_waypoint(&mut self.activity, route);
    }

    /// The Auto-mode screen follow (epic #506, C5), driven off the climb entry/exit edge in
    /// [`update_active_climb`](App::update_active_climb) — the host-pushed-screen pattern (the P2
    /// precedent the [`CardScheduler`](crate::card_scheduler::CardScheduler) now owns), applied to
    /// the active-climb transition rather than a route upload:
    ///
    /// - **Entry** (`None → Some`): in [`Auto`](crate::settings::ClimbMode::Auto) mode, if the top
    ///   screen is exactly Map or Statistics, switch it to the Climb screen. The explicit sibling
    ///   guard is the whole point: a rider deep in a menu, pause page, or an interactive map-based
    ///   chooser such as Skip ahead is never yanked out.
    /// - **Exit** (`Some → None`): replace a Climb screen anywhere in the stack with Map, without
    ///   dismissing chrome or an interactive chooser above it. Usually Climb is the top; the wider
    ///   repair matters when Ride menu or Skip ahead was opened from Climb before the crest. Either
    ///   way, returning later cannot reveal a stale "No climb" panel. This runs regardless of mode:
    ///   once the climb ends there's nothing for that screen to show.
    ///
    /// [`Manual`](crate::settings::ClimbMode::Manual) and [`Off`](crate::settings::ClimbMode::Off)
    /// never *enter*; the exit return still fires from Manual (the rider cycled to the Climb screen
    /// themselves), but not from Off (the Climb screen is out of the ring, so the top is never it).
    /// A `Replace` (not a push) so the ring's depth is unchanged — the Climb screen is a sibling of
    /// the riding views, not an overlay.
    fn apply_climb_auto_switch(&mut self, prev: Option<usize>, next: Option<usize>) {
        let top_is = |app: &Self, want: fn(&Screen) -> bool| app.ui.stack.last().is_some_and(want);
        match (prev, next) {
            // Entry: Auto + on one of the two eligible riding siblings → show the Climb screen.
            (None, Some(_))
                if self.settings.climb_mode == crate::settings::ClimbMode::Auto
                    && top_is(self, |s| matches!(s, Screen::Map(_) | Screen::Statistics(_))) =>
            {
                if let Some(top) = self.ui.stack.last_mut() {
                    *top = Screen::Climb(crate::screen::ClimbScreen::new());
                }
            }
            // Exit (crest): repair the caller in place, preserving any active menu/chooser above it.
            (Some(_), None) => {
                if let Some(climb) = self.ui.stack.iter_mut().rfind(|s| matches!(s, Screen::Climb(_))) {
                    *climb = Screen::Map(MapScreen::new());
                }
            }
            _ => {}
        }
    }

    /// Whether the base (lowest opaque) screen draws the **map** — any screen declaring
    /// [`BaseContent::Map`](crate::screen::BaseContent::Map). A render-on-demand host polls this to
    /// skip the whole map pipeline on a non-map frame: don't build the `Reader` (an SD style-table
    /// parse + its stack spike), pass `None` to
    /// [`render_map_timed`](App::render_map_timed), and a menu / Home redraw draws only its own
    /// chrome with zero map I/O.
    pub fn base_draws_map(&self) -> bool {
        self.ui.base_draws_map()
    }

    /// Whether the current base screen consumes a rain-raster lease. Hosts use this before
    /// constructing [`RainOverlayAdapter`](crate::RainOverlayAdapter), so its header/frame reads
    /// never happen on Home, menus, or the ordinary Map where the lease would be discarded.
    pub fn base_wants_rain(&self) -> bool {
        self.ui.base_wants_rain()
    }

    /// Whether the **Recalculating freeze** is engaged (issue #1146, P2): a host planner run is
    /// live *and* the base screen would draw the map. While it is, a render-on-demand host must
    /// **skip the map redraw** — the last frame stays on the reflective glass — and paint only
    /// [`render_overlay`](App::render_overlay), which raises the "Recalculating..." banner over it.
    /// [`tick`](App::tick) stops advancing route-match progress for the same span (everything else
    /// about a fix keeps recording).
    ///
    /// The board reads it once per pass for two decisions: whether to render, and whether the nav
    /// arm of the scratch arena may be claimed — the map plane must already be quiet before a
    /// search overwrites the render scratch, so this is the proof
    /// [`nav_arena_precondition`](App::nav_arena_precondition) hands to
    /// [`ArenaGate::claim_nav`](crate::arena_gate::ArenaGate::claim_nav).
    pub fn reroute_freeze_active(&self) -> bool {
        self.mode.frozen(self.ui.base_draws_map())
    }

    /// What the device is busy with, ranked and payload-free — [`CoreMode`]'s one public read.
    /// (Named apart from [`mode`](App::mode), which is the rider's *activity* — Idle or Riding.)
    ///
    /// A search outranks a transfer because it is the one the rider is waiting on and the one with
    /// a banner. Admission never reads this: it reads the levels, so the ranking cannot hide one
    /// behind the other.
    pub fn core_mode(&self) -> ModeState {
        self.mode.state()
    }

    /// The proof that the map plane is quiesced, minted from this app's own state — `None` when a
    /// search must not take the arena yet (a map base with no freeze engaged). The board's
    /// `claim_nav` call site is `app.nav_arena_precondition().ok_or(…)?`, so the gate cannot be
    /// called without the evidence.
    pub fn nav_arena_precondition(&self) -> Option<crate::arena_gate::MapQuiesced> {
        self.mode.nav_precondition(self.base_draws_map())
    }

    /// The proof that a cable upload may take the arena's staging arm: the transfer card is up
    /// (`render ⊥ usb`) and no search holds the nav arm (`nav ⊥ usb`). The board's `claim_usb` call
    /// site reads this rather than assembling the two facts itself.
    pub fn usb_stage_precondition(&self) -> Option<crate::arena_gate::TransferReady> {
        self.mode.usb_precondition(self.map_transfer_card_up())
    }

    /// Whether the frame needs the streamed-map [`Reader`] built and passed to
    /// [`render_map_timed`](App::render_map_timed) — a superset of [`base_draws_map`](App::base_draws_map).
    /// Map-base screens always do; the **POI list** screen (issue #425) does too, but only until it
    /// has taken its one-shot snapshot; and the **POI detail** screen (issue #444) does until it has
    /// resolved its one hours read. The POI screens read the `Reader` in their pre-draw prepare
    /// pass, so a render-on-demand host (the board's two-plane loop) must build it on the frame each
    /// one-shot read is taken. Once the list's [`poi_snapshot_pending`](App::poi_snapshot_pending)
    /// is false — or the detail's schedule cache has resolved — the screen draws from its frozen
    /// state with no `Reader`, so the host skips the build again.
    ///
    /// The sim's `render_frame` always passes `Some(reader)`, so it never consults this — only the
    /// board host does, keeping its per-frame `Reader` build (and stack spike) off every non-map,
    /// already-resolved frame.
    pub fn base_needs_reader(&self) -> bool {
        self.ui.base_needs_reader()
    }

    /// Whether there's a **current** GPS fix at `now_ms`: a fix has been accepted and is no older
    /// than the ride engine's staleness window ([`RideEngine::has_live_fix`]). `false` before the
    /// first fix (acquiring) and once the signal drops (lost) — exactly when the "No GPS Fix"
    /// banner shows.
    pub fn has_live_fix(&self, now_ms: u32) -> bool {
        self.ride.has_live_fix(now_ms, &self.settings)
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
        self.ui.map_dirty = true;
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

    /// Declare whether this platform's panel has a controllable light — the host asks its
    /// [`Backlight`](obc_ports::Backlight) port ([`available`](obc_ports::Backlight::available))
    /// once at composition and states the answer here.
    ///
    /// `false` **removes** the quick drawer's brightness control, leaving three icons. A control
    /// the hardware cannot honour is worse than no control: a slider that moves, a check-mark that
    /// relocates and a setting that persists, with no photons — the same lie the port refuses to
    /// tell, moved to the screen. See the deviation note in `screen/quick_drawer.rs`.
    pub fn set_backlight_available(&mut self, available: bool) {
        if self.backlight_available != available {
            self.backlight_available = available;
            self.ui.map_dirty = true;
        }
    }

    /// Whether the panel has a controllable light (see
    /// [`set_backlight_available`](App::set_backlight_available)).
    pub fn backlight_available(&self) -> bool {
        self.backlight_available
    }

    /// The loaded map's resident routing-profile names (read-only), for host inspection / tests.
    pub fn nav_profiles(&self) -> &crate::NavProfiles {
        &self.nav_profiles
    }

    /// Replace the resident route catalog from the host's store, carrying each route's **durable
    /// object id** (`ids` parallel to `summaries`), then remap every held catalog index by id
    /// (#450). Clones up to [`MAX_ROUTES`](crate::MAX_ROUTES) entries; any beyond that are ignored.
    ///
    /// The remap is the live-catalog contract: a rescan that inserts or removes a route re-points
    /// [`Activity::active_route`], the matcher/profile caches keyed on it, an open Skip-ahead
    /// chooser or queued skip commit, a Route-menu selection, a Route-overview preview, and a pending
    /// [`RouteSwapScreen`](crate::screen::RouteSwapScreen) at the *same route* (by id) in the new
    /// order. A vanished route falls back sanely: navigation unloads (`active_route = None`, stale
    /// matcher progress + profile dropped), a menu selection clamps near its old position, a
    /// preview/swap subject turns into its screen's own missing-route path. Dirties the map once —
    /// a store change is a repaint-worthy host event (the open menu refreshes in place).
    pub fn set_routes_with_ids(&mut self, summaries: &[RouteSummary], ids: &[crate::CatalogObjectId]) {
        // The catalog + trip replacement (and the id ↔ summary pairing) is `CatalogState`'s; the
        // old-id snapshot it returns drives the remap of everything held *outside* it.
        let old_ids = self.catalogs.replace_routes(summaries, ids);
        self.remap_route_indices(&old_ids);
        self.ui.map_dirty = true;
    }

    /// [`set_routes_with_ids`](App::set_routes_with_ids) **plus** the host's fresh per-route
    /// retention metas (read from the SD route-retention sidecar, epic #638 S3), pairwise with
    /// `ids`. The base call remaps held indices and carries surviving routes' metas across by
    /// identity; this then overlays the host's device-durable retention values so the sweep reads
    /// device truth. Retention-aware hosts (the board, the simulator) call this; plain
    /// [`set_routes_with_ids`](App::set_routes_with_ids) callers leave every route at the safe
    /// default ([`Never`](crate::Retention::Never) — nothing expires).
    pub fn set_routes_with_meta(
        &mut self,
        summaries: &[RouteSummary],
        ids: &[crate::CatalogObjectId],
        metas: &[crate::retention::RouteRetentionMeta],
    ) {
        self.set_routes_with_ids(summaries, ids);
        self.catalogs.set_route_meta(metas);
    }

    /// Each resident route's retention meta, pairwise with [`route_ids`](App::route_ids) (epic #638
    /// S3) — the host's read-back (e.g. to keep a sidecar row aligned) and the sweep tests' probe.
    pub fn route_metas(&self) -> &[crate::retention::RouteRetentionMeta] {
        self.catalogs.route_metas()
    }

    /// Overlay the host's fresh per-route retention metas (from the SD sidecar), pairwise with the
    /// **current** [`route_ids`](App::route_ids) — the standalone meta feed a host calls when it
    /// re-reads the sidecar without replacing the catalog (the sim re-pushes it each frame so the
    /// sweep always mirrors device truth). No catalog replacement, no remap. Excess metas are ignored.
    pub fn set_route_meta(&mut self, metas: &[crate::retention::RouteRetentionMeta]) {
        self.catalogs.set_route_meta(metas);
    }

    /// Re-point every held catalog index after the catalog was replaced: old index → its id in
    /// `old_ids` → that id's new index (or `None` if the route vanished). See
    /// [`set_routes_with_ids`](App::set_routes_with_ids).
    fn remap_route_indices(&mut self, old_ids: &[crate::CatalogObjectId]) {
        let App { catalogs, ride, activity, ui, navigator, .. } = self;
        let remap = |i: usize| -> Option<usize> { catalogs.remap_route(old_ids, i) };

        // The navigated route + every ride-engine cache keyed on it follow the identity together
        // (survives → nothing resets; vanished → navigation unloads and the stale per-route state
        // drops with it) — the ride engine owns that walk.
        ride.remap_route_keys(activity, &remap);
        // The undelivered detour request follows the same durable identity as the route it plans
        // around, or is dropped with it — Navigator's half of the same walk.
        navigator.remap_detour_route(&remap);

        // Every screen on the stack that holds a catalog index. The Route menu also takes the
        // re-resolved trips (`replace_routes` re-filed them before returning) + the new route count
        // so it can follow its highlight into the regrouped (folders + unfiled routes) list.
        let new_len = catalogs.route_len();
        let trips = catalogs.trips();
        for s in ui.stack.iter_mut() {
            match s {
                Screen::RouteMenu(m) => m.remap_routes(&remap, trips, new_len),
                Screen::RouteOverview(o) => o.remap_routes(&remap),
                Screen::RouteSwap(sw) => sw.remap_routes(&remap),
                Screen::RouteReceived(rc) => rc.remap_routes(&remap),
                Screen::RouteUpdated(ru) => ru.remap_routes(&remap),
                Screen::Detour(d) => d.remap_routes(&remap),
                Screen::DetourPreview(p) => p.remap_routes(&remap),
                _ => {}
            }
        }
    }

    /// The resident route catalog.
    pub fn routes(&self) -> &[RouteSummary] {
        self.catalogs.routes()
    }

    /// Each catalog entry's durable object id, pairwise with [`routes`](App::routes) — as last fed
    /// to [`set_routes_with_ids`](App::set_routes_with_ids) (positional for plain
    /// [`set_routes`](App::set_routes)).
    pub fn route_ids(&self) -> &[crate::CatalogObjectId] {
        self.catalogs.route_ids()
    }

    /// The active route's catalog index, or `None` when no route is loaded — the read a host uses to
    /// sync its route store's active bytes each pass (the write twin is the menu selection / a
    /// finished plan, never a host poke of the field).
    pub fn active_route_index(&self) -> Option<usize> {
        self.activity.active_route
    }

    /// Activate the route at catalog index `idx` (a host baseline / demo-reset seam) — the
    /// invariant-preserving twin of the menu's own selection, bounds-checked against the resident
    /// catalog, so hosts never write `activity.active_route` directly to stage a route. An
    /// out-of-range index clears the active route. Dirties the map so an open Map repaints the line.
    pub fn activate_route(&mut self, idx: usize) {
        self.activity.active_route = (idx < self.catalogs.route_len()).then_some(idx);
        self.ui.map_dirty = true;
    }

    /// Replace the resident **trip** catalog from the host's store (epic #526, TR2). Each
    /// [`TripInput`](crate::trip::TripInput) carries the trip's durable id, name, and stage route ids;
    /// the app resolves the ids against the current route catalog (`catalog_ids`) into a
    /// [`TripSummary`](crate::trip::TripSummary) — resolved catalog indices in ride order + summed
    /// distance/climb over the resolvable stages, dangling refs dropped. Clones up to
    /// [`MAX_TRIPS`](crate::MAX_TRIPS) trips; any beyond that are ignored (the host warns + lists the
    /// first N, mirroring the route-scan overflow). Call **after** the routes are set so the stage ids
    /// resolve; a later [`set_routes_with_ids`](App::set_routes_with_ids) re-resolves them in place.
    /// Dirties the map so an open (TR3) menu repaints.
    pub fn set_trips(&mut self, trips: &[crate::trip::TripInput]) {
        self.catalogs.set_trips(trips);
        self.ui.map_dirty = true;
    }

    /// The resident trip catalog (epic #526) — the grouped-route folders. The TR3 Route menu lists
    /// these above the unfiled routes; until then they're resolved but unrendered.
    pub fn trips(&self) -> &[crate::trip::TripSummary] {
        self.catalogs.trips()
    }

    /// Whether the route at catalog index `idx` is **filed** into some trip (epic #526) — a filed
    /// route shows only inside its folder, so the TR3 top level lists trips + unfiled routes. Until
    /// TR3 the flat menu ignores this and lists every route.
    pub fn route_filed(&self, idx: usize) -> bool {
        self.catalogs.route_filed(idx)
    }

    /// Replace the resident **ride** catalog from the host's store (epic #447, P7), carrying each
    /// ride's durable object id (`ids` parallel to `summaries`) and its `synced` flag (baked into the
    /// summary by the host from the SD synced-set sidecar). Re-points an open Rides-menu selection by
    /// id across the rescan, so a finished ride, a phone-side ride delete, or an on-device delete
    /// appears/disappears without a reboot. Clones up to [`MAX_RIDES`](crate::MAX_RIDES); any beyond
    /// that are ignored. Sorted-by-`start_time` is the host's job (the board scan and the sim store
    /// both hand newest-first). Dirties the map once — a store change is a repaint-worthy event.
    pub fn set_rides(&mut self, summaries: &[RideSummary], ids: &[crate::CatalogObjectId]) {
        // Re-point every held ride index by identity (its id in `old_ids` → new index), the
        // ride-namespace twin of the route remap: `replace_rides` moves its own view-cache keys
        // (the profile/preview the detail's band hangs off — identity survives → the resident
        // profile moves with it, no re-stream; vanished → the buffer drops); the Rides menu's
        // highlight, an open Ride detail's subject (#680 — a vanished subject becomes the detail's
        // missing-ride state), and the viewed-ride key are remapped here with the same old ids.
        let old_ids = self.catalogs.replace_rides(summaries, ids);
        let catalogs = &self.catalogs;
        let remap = |i: usize| -> Option<usize> { catalogs.remap_ride(&old_ids, i) };
        let new_len = catalogs.ride_len();
        for s in self.ui.stack.iter_mut() {
            match s {
                Screen::Rides(m) => m.remap_rides(&remap, new_len),
                Screen::RideDetail(d) => d.remap_rides(&remap),
                _ => {}
            }
        }
        self.activity.viewed_ride = self.activity.viewed_ride.and_then(remap);
        self.ui.map_dirty = true;
    }

    /// Feed the **full** compact ride-retention inventory (finding #876-2): every stored ride's
    /// `id + synced + synced_at`, up to [`MAX_RIDES`](crate::MAX_RIDES), independent of the
    /// newest-32 UI catalog [`set_rides`](App::set_rides) carries. A retention-aware host (the board)
    /// streams this from its whole-store synced-set after each rescan so the auto-delete sweep + the
    /// eager `synced_at` stamp reach a synced+expired ride even when it never sits in the display
    /// list. Call **after** [`set_rides`](App::set_rides) (which seeds a display-only fallback).
    pub fn set_ride_retention_inventory(&mut self, records: &[crate::retention::RideRetentionRecord]) {
        self.catalogs.set_ride_retention_inventory(records);
    }

    /// The resident ride catalog (summaries) — what the Rides screen lists.
    pub fn rides(&self) -> &[RideSummary] {
        self.catalogs.rides()
    }

    /// Each ride-catalog entry's durable object id, parallel to [`rides`](App::rides) — as last fed to
    /// [`set_rides`](App::set_rides).
    pub fn ride_ids(&self) -> &[crate::CatalogObjectId] {
        self.catalogs.ride_ids()
    }

    /// Borrow the app's one resident ride-profile buffer for an in-place host fill. **Invalidates**
    /// the ride-track view: until a keyed answer for the *post-fill* key lands
    /// ([`apply_derived`](App::apply_derived)) the level re-fires, so an abandoned fill leaves a
    /// need up rather than a half-written buffer marked answered.
    ///
    /// **Temporary wrapper — deleted by DC6 #1439.**
    pub fn begin_ride_profile_fill(&mut self) -> &mut Profile {
        self.catalogs.begin_ride_profile_fill()
    }

    /// Open the on-glass DFU check flow from a **remote** request — the BLE `installFw` command
    /// (epic #615 S6, #621): push the "Checking card..." wait and post
    /// [`DfuAction::Scan`](crate::activity::DfuAction), exactly the System menu's press arriving
    /// over the air. **Never `Install`** — a remote request can only open the scan → confirm flow;
    /// the Select press on the confirm screen is what posts the arm (spec §4.4: the phone can
    /// request, only the rider installs; the direct-Install path stays the physical debug link's).
    ///
    /// Returns `true` when the flow opened (the board consumes its pending request); `false`
    /// **defers** — the board keeps the request pending and retries next pass, so an inconvenient
    /// moment delays the card, never drops or force-installs it. Deferred while:
    /// - the passkey card is up or a hold is charging (the
    ///   [`CardScheduler`](crate::card_scheduler::CardScheduler) politeness — never cover the
    ///   pairing code, never land mid-hold),
    /// - a DFU screen (check / confirm / progress / error) is already on the stack — never
    ///   double-open, and never yank a flow the rider opened from the menu themself,
    /// - a [`DfuAction`] is already posted but undrained (don't overwrite a phase in flight),
    /// - a ride is recording (defensive: the BLE edge already answered `busy`, but recording can
    ///   start between that reply and this drain).
    pub fn open_remote_dfu_check(&mut self) -> bool {
        let dfu_screen_up = self.ui.stack.iter().any(|s| {
            matches!(s, Screen::DfuCheck(_) | Screen::DfuConfirm(_) | Screen::DfuProgress(_) | Screen::DfuError(_))
        });
        if self.passkey_card_up()
            || self.ui.hold_charging()
            || dfu_screen_up
            || self.dfu.request_pending()
            || self.recorder.recording()
        {
            return false;
        }
        self.dfu.admit_intent(crate::dfu::DfuIntent::ScanRequested);
        let r = self.ui.stack.push(Screen::DfuCheck(crate::screen::DfuCheckScreen::new()));
        debug_assert!(r.is_ok(), "screen stack overflow — raise MAX_DEPTH");
        self.ui.map_dirty = true;
        true
    }

    /// **Debug bench** (#500): start a route plan from `from` to `to` (both `(lon, lat)` µdeg) exactly
    /// as the POI create-route confirm does — record the [`NavRequest`](crate::activity::NavRequest)
    /// **and** push the planning screen — so the host steps the resumable router with the same live
    /// spinner + between-step render cadence the rider sees, and the `nav route:` RTT line reflects the
    /// real user-perceived cost. Only wired on the `debug-uart` build (driven by the `N` VCOM command);
    /// no UI path reaches it. Returns `false` without changing pending state while a plan is active.
    pub fn debug_start_nav(&mut self, from: (i32, i32), to: (i32, i32), name: &str) -> bool {
        // At most one planning screen, ever: the bench host repeats the `N` line (the VCOM RX is
        // flaky). Reject a repeat before touching the request slot: once the host has drained the
        // first request, overwriting the resident planner would orphan its allocation and strand
        // this screen even though no second screen was pushed.
        if self.ui.stack.iter().any(|s| matches!(s, Screen::NavPlanning(_))) {
            return false;
        }
        self.admit_navigator_intent(NavigatorIntent::PlanRoute(crate::activity::NavRequest::new(from, to, name)));
        let _ = self.ui.stack.push(Screen::NavPlanning(crate::screen::NavPlanningScreen::new(name)));
        self.ui.map_dirty = true;
        true
    }

    /// **Debug only**: arm an install exactly as the confirm screen's press does, for the board's
    /// physical `dfu-install` VCOM command (#620) — which deliberately skips the confirm.
    ///
    /// It names the intent to [`DfuState`](crate::dfu::DfuState) rather than reaching for the
    /// executor, so the debug path and the rider's path produce the *same*
    /// [`DfuEffect::ArmInstall`](crate::dfu::DfuEffect) under the same operation token. A typed
    /// executor has no other way to mint one — the token source is the domain's.
    pub fn debug_request_dfu_install(&mut self) {
        self.dfu.admit_intent(crate::dfu::DfuIntent::InstallRequested);
    }

    /// **Debug / snapshot only** (#1146 P2): engage the Recalculating freeze as if the host had just
    /// begun a planner run — the same seam a drained `PlanRoute`/`PlanDetour` takes, so the banner,
    /// the paused matcher and the skipped redraws are the real ones.
    ///
    /// It exists because the freeze's visible state is not reachable from a *scripted* headless
    /// run: the flows that start a plan leave an opaque planning screen as the base (nothing to
    /// freeze), and the one gesture that puts a map base back under a live search — Back on the
    /// detour spinner — also cancels the plan, which the host drains in the same pass. The
    /// simulator's `--freeze` flag drives this so the banner can be snapshotted over a live map.
    /// No production path reaches it. Stands in for a [`Route`](PlanFamily::Route) run, so a stray
    /// detour edge cannot release it (see [`PlanFamily`]).
    pub fn debug_set_plan_live(&mut self, live: bool) {
        if self.navigator.debug_set_plan_live(live, &mut self.mode) {
            self.ui.map_dirty = true;
        }
    }

    /// Host-push the **weather alert card** (WX11, epic #1185): RAIN AHEAD / STORM AHEAD with the
    /// locked VIEW RAIN MAP + DISMISS actions. An alert already on the stack is *updated* in
    /// place (re-fires never stack cards); the passkey card outranks it (the pairing prompt is
    /// never covered — the upload-popup family's rule). Alert *generation* — thresholds, dedup,
    /// cooldown persistence — is WX12's; this is only the presentation seam it (and the sim's
    /// injection flag) drives.
    ///
    /// **Returns whether the alert actually reached the rider** — updated in place, or pushed.
    /// `false` means it was refused (a passkey prompt on top, or a screen stack already at
    /// [`MAX_DEPTH`](crate::screen::MAX_DEPTH)), and the caller must *not* record it as fired:
    /// writing a dedup mark for a card nobody saw would sit on the storm for a whole persisted
    /// cooldown in silence (review F4).
    pub fn show_weather_alert(&mut self, kind: crate::screen::WeatherAlertKind, minutes: u16) -> bool {
        for scr in self.ui.stack.iter_mut() {
            if let Screen::WeatherAlert(alert) = scr {
                if alert.update(kind, minutes) {
                    self.ui.map_dirty = true;
                }
                return true;
            }
        }
        if matches!(self.ui.stack.last(), Some(Screen::Passkey(_))) {
            return false;
        }
        // The stack's own capacity, checked here rather than left to `apply`'s push: an overflow
        // there no-ops silently in release (and trips a debug assert in test builds), so the one
        // caller that must *know* asks first.
        if self.ui.stack.len() >= crate::screen::MAX_DEPTH {
            return false;
        }
        // Whether the card lands over an already-open rain map — its VIEW RAIN MAP action then
        // pops back to it instead of stacking a second one (review F4).
        let over_rain_map = matches!(self.ui.stack.last(), Some(Screen::WeatherRainMap(_)));
        crate::screen::apply(
            &mut self.ui.stack,
            crate::screen::Transition::Push(Screen::WeatherAlert(crate::screen::WeatherAlertScreen::new(
                kind,
                minutes,
                over_rain_map,
            ))),
        );
        self.ui.map_dirty = true;
        true
    }

    /// The rider's current travel direction (degrees CW from north) for route-relative wind — the
    /// WX12 chain: the active route's general heading ahead of the matched progress while
    /// on-route, else `None` (neutral arrows, never a fabricated head/tail — a momentary GPS
    /// course is not a direction the rider is committed to).
    pub fn travel_deg(&self) -> Option<f32> {
        self.ride.travel_deg
    }

    /// The WX12 ride projection for [`WeatherSnapshot::sample_along`](crate::weather::WeatherSnapshot::sample_along),
    /// or `None` when there is no matched active route (the host then samples at the fixed rider
    /// position — WX11's behaviour). Pace = the recent moving median, capped, with the documented
    /// touring fallback while stopped; anchored at this instant's wall clock.
    ///
    /// **Off-route is `None`**, for the same reason [`travel_deg`](Self::travel_deg) switches to
    /// the GPS course there: `progress_m` is the last match on a line the rider has left, so
    /// projecting along it would answer the two-hour question about a route they aren't riding —
    /// 20 km away, in the wrong weather. Falling back to rider-position sampling is the honest,
    /// less-informative answer (review F1).
    pub fn ride_projection(&self) -> Option<crate::weather::RideProjection> {
        if self.activity.active_route.is_none() || !self.ride.started() || self.activity.off_route {
            return None;
        }
        let speed_cms = self
            .recorder
            .speed_win
            .median_cms()
            .unwrap_or(crate::weather::TOURING_FALLBACK_CMS)
            .min(crate::weather::SPEED_CAP_CMS);
        Some(crate::weather::RideProjection {
            progress_m: self.activity.progress_m,
            speed_cms,
            now: self.wall_unix_now() as i64,
        })
    }

    /// Run the WX12 **alert engine** against the pass's snapshot: evaluate the centralized
    /// threshold table, dedup against the persisted per-class marks, and drive the WX11 card seam —
    /// a new (or materially escalated) event pushes/re-fires the card and persists its mark through
    /// the #810 settings handshake; the same suppressed event only refreshes an already-open card's
    /// countdown in place. Cheap (a bounded scan), idempotent, deterministic. `None` (no snapshot)
    /// never alerts, and neither does expired data (the engine's law).
    ///
    /// **The production caller is stage 10** ([`stage_weather`](crate::device_core::PassStage::Weather)),
    /// once per pass. This stays a named method so tests and the simulator's `--weather-decide`
    /// still-frame path can drive the decision directly — an executor must not, or *when* the
    /// honesty law runs becomes its choice again.
    ///
    /// **A mark is written only for a card the rider actually saw.** `show_weather_alert` can
    /// refuse — a passkey prompt outranks it, and so does a screen stack already at `MAX_DEPTH` —
    /// and marking a refused alert would suppress that storm for a whole *persisted* cooldown with
    /// no card ever shown. So the refusal is read back, not assumed away (review F4); the next
    /// tick, once the stack has room again, re-fires.
    pub fn weather_alert_tick(&mut self, snap: Option<&crate::weather::WeatherSnapshot>) {
        use crate::weather_alerts::AlertAction;
        let now = self.wall_unix_now() as i64;
        let open_card = self.ui.stack.iter().find_map(|s| match s {
            Screen::WeatherAlert(alert) => Some(alert.kind()),
            _ => None,
        });
        // The decision is [`WeatherDomain`]'s (#1437): thresholds, dedup and cooldown all live
        // there. What is left here is the presentation seam and the persistence handshake.
        match self.weather.alert_action(snap, now, open_card) {
            AlertAction::Fire(c) => {
                if self.show_weather_alert(c.class.kind(), c.minutes) {
                    self.weather.mark_fired(&c);
                    // The mark must survive the next boot: arm the marks record's own handshake.
                    // Not the preferences one — a storm is not a rider edit, so it neither rewrites
                    // the preferences blob nor waits for the rider to leave a settings screen.
                    self.alert_marks_ops.note_edited();
                }
            }
            AlertAction::Update(c) => {
                self.show_weather_alert(c.class.kind(), c.minutes);
            }
            AlertAction::None => {}
        }
    }

    /// Drop **everything derived from the active route's geometry** — the whole-App seam, and the
    /// only thing route-replacing paths should call.
    ///
    /// `RideEngine::drop_route_derived_state` covers the engine's
    /// half (matcher, profile, climbs, waypoints, progress); the UI's
    /// [`NextAhead`](crate::next_ahead::NextAhead) cache is the other half and lives on the far
    /// side of the `ride`/`ui` split, so it cannot be reached from there. It is invalidated for
    /// exactly the same reason as the rest: its entries are **along-route distances**, and a
    /// same-index/new-bytes replace leaves the catalog index untouched — so the cache's own
    /// route-identity check sees nothing change, and an entry measured on the old geometry would
    /// name a different place on the new.
    pub(crate) fn drop_route_derived_state(&mut self) {
        self.ride.drop_route_derived_state(&mut self.activity);
        self.ui.next_ahead.invalidate();
    }

    /// Hand one rider request to Navigator, and repaint.
    ///
    /// The map is dirtied on any navigation intent because every screen that produces one is
    /// changing what the rider is looking at. A plan **start** deliberately dirties nothing extra:
    /// the executor is about to stop redrawing the map, and the banner's edge is the engaged
    /// *level*, which [`take_dirty`](App::take_dirty) derives (a plan begun under the opaque
    /// planning spinner freezes nothing at all).
    pub(crate) fn admit_navigator_intent(&mut self, intent: NavigatorIntent) {
        let planned = self.navigator.detour_planned();
        self.navigator.admit_intent(intent);
        self.sync_detour_preview(planned);
        self.ui.map_dirty = true;
    }

    /// Drop the detour preview polyline when Navigator drops the plan it previews.
    ///
    /// The shape is *derived* from the plan: it is drawn over the still-active route, so a preview
    /// of a detour that no longer exists is a line to nowhere. `was_planned` is the level from
    /// before the intent, so this fires on the falling edge and never on a boot with nothing cached.
    fn sync_detour_preview(&mut self, was_planned: bool) {
        if was_planned && !self.navigator.detour_planned() {
            self.catalogs.clear_detour_preview();
        }
    }

    /// Consume a typed [`NavigatorOutcome`](crate::navigator::NavigatorOutcome). The token is the
    /// whole admission test: a cancelled or superseded operation refuses its own late answer, and
    /// nothing downstream runs.
    ///
    /// What each accepted answer *means* to the rider is the same code the legacy events reach —
    /// there is one `land_*` per product event, not one per protocol.
    pub(crate) fn apply_navigator_outcome(&mut self, outcome: crate::navigator::NavigatorOutcome) {
        use crate::navigator::{NavigatorError, NavigatorOutcome};
        if !self.navigator.accepts(&outcome) {
            return;
        }
        match outcome {
            NavigatorOutcome::PlanFinished { route, .. } => self.land_route_plan(Ok(route)),
            NavigatorOutcome::DetourFinished { preview, .. } => self.land_detour_plan(Ok(preview)),
            NavigatorOutcome::DetourCommitted { route, .. } => self.land_detour_commit(Ok(route)),
            NavigatorOutcome::Failed { error, .. } => {
                // The planner's own verdict is the one the rider is shown; the two resource
                // failures have no tier of their own and land on the generic card, which is what
                // the legacy protocol has always done with them.
                let error = match error {
                    NavigatorError::Plan(error) => error,
                    NavigatorError::Workspace | NavigatorError::Store => obc_route::nav::NavError::NoPath,
                };
                match self.navigator.live_family() {
                    Some(PlanFamily::Detour) if self.navigator.detour_committing() => {
                        self.land_detour_commit(Err(error))
                    }
                    Some(PlanFamily::Detour) => self.land_detour_plan(Err(error)),
                    _ => self.land_route_plan(Err(error)),
                }
            }
            // The workspace came back, or the operation was abandoned: the run is over and the
            // freeze must not outlive it, but there is nothing new to put in front of the rider.
            NavigatorOutcome::Released { .. } | NavigatorOutcome::Cancelled { .. } => {
                let family = self.navigator.live_family().unwrap_or(PlanFamily::Route);
                self.end_plan(family, PlanPhase::Idle);
            }
            // Pacing is #1400's — one request here acquires, steps and
            // commits inside the executor, so no protocol in this slice produces these. They are
            // accepted and change nothing until #1397 S6 gives Navigator the stepping loop.
            NavigatorOutcome::Acquired { .. } | NavigatorOutcome::Stepped { .. } => {}
        }
    }

    /// Consume a typed [`DfuOutcome`](crate::dfu::DfuOutcome) — the same terminal cards the legacy
    /// events post, behind the token that says this answer is still the phase being waited for.
    pub(crate) fn apply_dfu_outcome(&mut self, outcome: crate::dfu::DfuOutcome) {
        use crate::dfu::DfuOutcome;
        if !self.dfu.accepts(&outcome) {
            return;
        }
        self.dfu.note_answer();
        match outcome {
            DfuOutcome::ScanFinished { report, .. } => self.post_dfu_landing(DfuLanding::Scanned(Ok(report))),
            DfuOutcome::ScanFailed { error, .. } => self.post_dfu_landing(DfuLanding::Scanned(Err(error))),
            DfuOutcome::InstallBegan { .. } => self.post_dfu_landing(DfuLanding::InstallBegan),
            DfuOutcome::InstallFailed { error, .. } => self.post_dfu_landing(DfuLanding::InstallFailed(error)),
            // An abandoned phase leaves the rider where they were: the wait screen is still up and
            // the menu still works, which is more honest than a failure card for work never done.
            DfuOutcome::Cancelled { .. } => {}
        }
    }

    /// Note a terminal planner answer for `family` and repaint the map the freeze held still.
    ///
    /// Dirties the **map**: it held still for the whole search and has a fix, a route, or a whole
    /// new geometry to catch up on. The banner comes off with the same
    /// [`take_dirty`](App::take_dirty) level edge that put it up. Idempotent: several release edges
    /// can land for one run.
    fn end_plan(&mut self, family: PlanFamily, phase: PlanPhase) {
        if self.navigator.note_answer(family, phase, &mut self.mode) {
            self.ui.map_dirty = true;
        }
    }

    /// The UI's reaction to Navigator finishing a **route** plan: land it in the planning screen,
    /// or drop it. Not a protocol handler — both protocols reach it through Navigator, which has
    /// already decided that this answer is the one being waited for.
    fn land_route_plan(&mut self, result: Result<crate::CatalogObjectId, obc_route::nav::NavError>) {
        use obc_route::nav::NavError;
        // The run is over whatever happens below — including for a *late* answer whose planning
        // screen the rider already cancelled away, which returns early two lines down.
        self.end_plan(PlanFamily::Route, if result.is_ok() { PlanPhase::Active } else { PlanPhase::Failed });
        let Some(i) = self.ui.stack.iter().position(|s| matches!(s, Screen::NavPlanning(_))) else {
            return;
        };
        // Resolve the id in the (already rescanned) catalog; a missing id degrades to the
        // generic failure tier.
        let resolved = result.and_then(|id| self.catalogs.route_index_of(id).ok_or(NavError::NoPath));
        let screen = match resolved {
            Ok(idx) => {
                // New bytes may sit under a same-id reserved file (a re-route): drop everything
                // derived from the old geometry so the matcher re-locks and the profile rebuilds
                // from the fresh route — cheap, runs once per plan.
                self.drop_route_derived_state();
                // Activate for the preview (the overview contract: the host streams the geometry
                // while the page shows); `prev_active` restores whatever was loaded on cancel.
                let prev = self.activity.active_route;
                self.activity.active_route = Some(idx);
                // Every plan starts preview-less (#685 §4): a re-route commits new bytes under
                // the same id/index, so an old shape must never survive into the new overview.
                // The host hands the fresh decimated polyline via `set_nav_preview` (the sim's
                // commit tail does it in the same pass; the board on the next one).
                self.catalogs.invalidate_nav_preview();
                Screen::RouteOverview(crate::screen::RouteOverviewScreen::computed(idx, prev))
            }
            // Exhaustion is the device's honest "too far" — the range tier's trigger now that
            // there is no crow-flies cap; everything else is the generic tier.
            Err(NavError::Exhausted) => Screen::NavFail(crate::screen::NavFailScreen::too_far()),
            Err(_) => Screen::NavFail(crate::screen::NavFailScreen::not_found()),
        };
        self.ui.stack[i] = screen;
        self.ui.map_dirty = true;
    }

    /// The detour plan's answer (#882): land it in the detour planning screen —
    /// success replaces it with the preview (cost line + the polyline handed in via
    /// [`set_detour_preview`](App::set_detour_preview)), failure with the fail card carrying the
    /// "try a farther rejoin" hint. A late answer whose planning screen is gone (the rider
    /// cancelled) is dropped, and the stale preview slot cleared.
    fn land_detour_plan(&mut self, result: Result<crate::host::DetourPreview, obc_route::nav::NavError>) {
        use obc_route::nav::NavError;
        // The run is over — see `land_route_plan` for the late-answer case.
        self.end_plan(PlanFamily::Detour, if result.is_ok() { PlanPhase::PreviewReady } else { PlanPhase::Failed });
        let Some(i) = self
            .ui
            .stack
            .iter()
            .position(|s| matches!(s, Screen::NavPlanning(p) if p.kind() == crate::screen::PlanKind::Detour))
        else {
            self.catalogs.clear_detour_preview();
            return;
        };
        // The chooser below the planning screen carries the request context the preview inherits.
        let chooser = self.ui.stack.iter().find_map(|s| match s {
            Screen::Detour(d) => Some(*d),
            _ => None,
        });
        let screen = match (result, chooser) {
            (Ok(preview), Some(d)) => Screen::DetourPreview(crate::screen::DetourPreviewScreen::new(&d, preview)),
            // No chooser below (stack surgery raced the answer): treat as the generic failure.
            (Ok(_), None) | (Err(NavError::NoPath), _) => {
                Screen::NavFail(crate::screen::NavFailScreen::detour_not_found())
            }
            (Err(NavError::Exhausted), _) => Screen::NavFail(crate::screen::NavFailScreen::detour_too_far()),
        };
        self.ui.stack[i] = screen;
        self.ui.map_dirty = true;
    }

    /// The splice's answer (#882): re-adopt the spliced route and land back on the
    /// riding view — or surface the failure on the preview with the old route fully intact.
    ///
    /// Success order matters: drop every cache derived from the old geometry, point
    /// `active_route` at the spliced route (the tracking session is deliberately untouched — the
    /// RouteSwap precedent), queue the seam re-anchor (the tick that owns the `RouteReader`
    /// installs matcher progress + floor at the splice seam), then truncate the detour flow off
    /// the stack so the rider lands on the exact riding view they left.
    fn land_detour_commit(&mut self, result: Result<crate::CatalogObjectId, obc_route::nav::NavError>) {
        self.navigator.note_commit(result.is_ok());
        let resolved = result.and_then(|id| self.catalogs.route_index_of(id).ok_or(obc_route::nav::NavError::NoPath));
        match resolved {
            Ok(idx) => {
                let anchor = self.ui.stack.iter().find_map(|s| match s {
                    Screen::DetourPreview(p) => Some(p.anchor_m()),
                    _ => None,
                });
                self.drop_route_derived_state();
                // The splice committed new geometry, often under the same route identity — the
                // derived keys move with the bytes (#1437).
                self.catalogs.note_commit();
                self.activity.active_route = Some(idx);
                self.activity.request_seam(idx, anchor.unwrap_or(0));
                self.catalogs.clear_detour_preview();
                if let Some(i) = self.ui.stack.iter().position(|s| matches!(s, Screen::Detour(_))) {
                    self.ui.stack.truncate(i.max(1)); // never below the Home root
                }
                self.ui.map_dirty = true;
            }
            Err(_) => {
                // The splice failed before anything was adopted: the old route + session are
                // untouched. Surface it inline on the preview (if it is still up).
                for s in self.ui.stack.iter_mut() {
                    if let Screen::DetourPreview(p) = s {
                        p.set_commit_failed();
                    }
                }
                self.ui.map_dirty = true;
            }
        }
    }

    /// Feed whether the loaded map carries a non-empty §8 nav graph (#882) — called once at map
    /// open by every host (`reader.nav_directory().is_empty()` is the source). Gates the ride
    /// menu's Detour station and the chooser.
    pub fn set_map_nav_graph(&mut self, present: bool) {
        self.state.has_nav_graph = present;
    }

    /// Hand in the planned detour's decimated polyline (#882) — the detour twin of
    /// [`set_nav_preview`](App::set_nav_preview), keyed to the active route the detour was
    /// planned against.
    pub fn set_detour_preview(&mut self, pts: &[(i32, i32)]) {
        self.catalogs.set_detour_preview(pts, self.activity.active_route);
        self.ui.map_dirty = true;
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
        self.derived_needs().nav_preview.is_some()
    }

    /// Hand in the previewed route's decimated shape polyline (#685 §4) — ≤
    /// [`NAV_PREVIEW_MAX`] `(lon, lat)` µdeg points (more are truncated), **decimated host-side**
    /// (the sim/web hosts' per-pass fill; the board's ride loop; a plan's commit tail).
    ///
    /// **Temporary wrapper — deleted by DC6 #1439.** Keyed to the previewed route's durable
    /// identity, the revision its bytes were last known to change at, and the view generation — so a
    /// route change, a re-plan over the same id, or a committed detour all stale it automatically.
    pub fn set_nav_preview(&mut self, pts: &[(i32, i32)]) {
        use crate::device_core::derived::{DerivedInput, DerivedInputs, DerivedTargets};
        let Some(key) = self.catalogs.nav_preview_key(self.activity.active_route) else { return };
        let input = DerivedInput::filled(key);
        self.apply_derived(
            DerivedInputs::nav_preview(input),
            DerivedTargets { nav_preview: pts, ..DerivedTargets::NONE },
        );
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
    /// unchanged status the card scheduler's sweep is a no-op, so the steady state never re-dirties.
    /// Because it's a host-pushed screen, it also **defers while a hold is charging** (yanking the
    /// hold target out from under the rider mid-charge would break the confirm) — the sweep just
    /// skips that pass and lands on the next, since the desired level is re-fed every pass.
    pub fn set_ble_status(&mut self, status: crate::ble::BleStatus) {
        let changed = (self.state.device.ble_link, self.state.device.ble_paired) != (status.link, status.paired);
        self.state.device.ble_link = status.link;
        self.state.device.ble_paired = status.paired;
        // **An explicit request, and it stays one.** The connected indicator is title-bar chrome on
        // every chrome-based screen, which is far more rows than the one that declares a key naming
        // it (Home) — and this seam is a host feeder a runtime may ring between two passes, where a
        // stack-local key comparison sees nothing anyway. Gated on the base screen actually drawing
        // the glyph, so a link change never forces a full map render on the riding views.
        if changed && self.ui.indicator_visible() {
            self.ui.map_dirty = true;
        }
        self.ui.cards.set_passkey(status.passkey);
        self.sweep_cards();
    }

    /// Whether the passkey card is currently up (epic #447). The P4 route-upload popups poll this to
    /// honour the priority rule — a popup is dropped, not queued, while the card shows.
    pub fn passkey_card_up(&self) -> bool {
        self.ui.passkey_card_up()
    }

    /// Run the one [`CardScheduler`](crate::card_scheduler::CardScheduler) sweep with the
    /// cross-component facts it needs. Called once per [`advance_animations`](App::advance_animations)
    /// pass, and again right after any host fact is posted so an arriving card lands in the same
    /// frame unless a policy rule defers it.
    fn sweep_cards(&mut self) {
        self.ui.run_card_sweep(&self.catalogs, self.recorder.recording());
    }

    /// The update domain's terminal answer — a scan result, the install beginning, or its failure:
    /// post it for the DFU wait
    /// on the stack. The scheduler drops it when that wait is gone (the rider pressed Back).
    pub(crate) fn post_dfu_landing(&mut self, landing: DfuLanding) {
        self.ui.cards.post_dfu(landing);
        self.sweep_cards();
    }

    /// This boot's update verdict: post this boot's
    /// one-time update verdict for the toast (or its failure twin).
    pub(crate) fn post_boot_update(&mut self, result: BootUpdate) {
        self.ui.cards.post_update(result);
        self.sweep_cards();
    }

    // ==================== map-transfer seam (issue #927) ====================

    /// Feed the board's live **map-transfer** state and reconcile the host-pushed card to it — the
    /// map twin of [`set_ble_status`](App::set_ble_status)'s passkey handling, and the only thing
    /// on glass during a write that runs for minutes.
    ///
    /// A map upload is unlike every other object the device accepts: hundreds of megabytes at the
    /// card's proven throughput saturate the SD bus for minutes, and the map plane's own reads queue
    /// behind it. Left unexplained that reads as a device that has gone sluggish or wedged, so the
    /// board publishes progress (through atomics the ride loop polls, hence a `Copy` value fed every
    /// pass rather than an event) and this raises the card.
    ///
    /// Idempotent: an unchanged state repaints nothing. `None` closes the card — which is also how
    /// an abort or an unplug ends it, deliberately: the rider caused those, and a red card
    /// explaining what they just did is noise. Only outcomes they can act on
    /// ([`MapTransfer::Installed`](crate::screen::MapTransfer::Installed) /
    /// [`Failed`](crate::screen::MapTransfer::Failed)) stay up to be dismissed.
    /// **The card only.** This seam used to also drive [`CoreMode`]'s transfer level, and that was
    /// the level's whole source — so a route, trip or weather upload streamed without admission ever
    /// seeing it, and a map upload paced to the card's own throttle reported the level at that pace.
    /// Since #1397 S6b the level comes from the store engine's own live transfer, through
    /// [`ExternalFacts::note_transfer`](crate::device_core::ExternalFacts::note_transfer), and this
    /// is a screen feeder like every other.
    ///
    /// [`CoreMode`]: crate::device_core::core_mode::CoreMode
    pub fn set_map_transfer(&mut self, state: Option<crate::screen::MapTransfer>) {
        self.ui.cards.set_map_transfer(state);
        self.sweep_cards();
    }

    /// Whether the map-transfer card is currently up (issue #927) — how a host observes the seam,
    /// and the query a future modal-priority rule would consult.
    pub fn map_transfer_card_up(&self) -> bool {
        self.ui.map_transfer_card_up()
    }

    /// The live BLE pairing passkey, or `None` when not pairing — [`BleStatus::passkey`](crate::BleStatus)
    /// as last fed to [`set_ble_status`](App::set_ble_status). Consumed by the passkey card in P2
    /// (#449); exposed now so the seam is observable end to end.
    pub fn ble_passkey(&self) -> Option<u32> {
        self.ui.cards.passkey_level()
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
        self.ui.set_sensor_status(status);
    }

    /// Feed the host's live **sensor scan hits** ([`SensorScanHit`](crate::sensors::SensorScanHit)) —
    /// the sensors discovered while the scan-list screen runs a scan. Replaces the resident list
    /// wholesale (up to [`SCAN_HITS_MAX`](crate::sensors::SCAN_HITS_MAX)); an empty slice clears it
    /// (the host feeds `&[]` when no scan is active). A change while the scan screen is up dirties the
    /// map so a freshly-found sensor appears without waiting for another input.
    pub fn set_sensor_scan_hits(&mut self, hits: &[crate::sensors::SensorScanHit]) {
        self.ui.set_sensor_scan_hits(hits);
    }

    /// Whether the rider is on the **scan-list** screen and a scan should run (SE7) — the level the
    /// Sensors screen raises on entry to a row and lowers on exit/Back
    /// ([`Activity::request_sensor_scan`](crate::activity::Activity)). The host reads it each pass (the
    /// `set_radio_enabled` shape): while `true` it keeps a discovery scan running and feeds the hits
    /// back; when it falls it clears the app scan list.
    pub fn sensor_scan_active(&self) -> bool {
        self.activity.sensor_scan_active()
    }

    /// A committed route upload: forced adoption on an active replace + the advisory prompt.
    pub(crate) fn on_route_uploaded(
        &mut self,
        id: crate::CatalogObjectId,
        replaced: bool,
        elevation: Option<[u8; obc_route::SPARKLINE_BUCKETS]>,
    ) {
        let active_id = self.activity.active_route.and_then(|i| self.catalogs.route_id_at(i));
        let active_replace = replaced && active_id == Some(id);
        if replaced {
            // New bytes under a durable identity: every derived key moves, so a preview or profile
            // produced from the old geometry stops matching. Identity alone cannot catch this one —
            // the id is exactly what did *not* change (#1437).
            self.catalogs.note_commit();
        }
        if active_replace {
            // Same index, same id — but new bytes. Invalidate everything derived from the old
            // geometry (the remap deliberately preserves same-id state; a replace is the one case
            // where that preservation would carry stale state onto new geometry).
            self.drop_route_derived_state();
            self.ui.map_dirty = true; // the drawn route line + progress changed under the rider
        }
        self.ui.cards.post_upload(PendingUpload::Route(UploadEvent { id, active_replace, elevation }));
        self.sweep_cards();
        // Anchor the route's retention clock at upload time (auto-expiry epic #638 S4): a fresh or
        // replace upload is a "use", so its expiry clock anchors here rather than at the next hourly
        // sweep. Whether the clock may be trusted is the domain's rule, applied inside
        // `note_route_uploaded` — the same place its sibling `note_route_activated` applies it.
        self.with_retention(|retention, view| retention.note_route_uploaded(id, view));
    }

    /// A committed trip upload: the "TRIP RECEIVED" advisory prompt — for a **fresh** trip
    /// only. Nothing to adopt or invalidate — a trip is a folder of already-committed routes (each
    /// of which raised its own event when it landed); this popup replaces the burst's last
    /// per-route popup, so one card announces the whole delivery. A **replace** is a trip *edit*
    /// pushed from the host (hosts edit a trip exclusively by replace-at-same-id — the desktop's
    /// rename / add / remove / reorder is one upload per click), so it is silent: the user just
    /// made the change, and a card per click would be the exact parade this event exists to kill.
    pub(crate) fn on_trip_uploaded(&mut self, id: crate::CatalogObjectId, replaced: bool) {
        if replaced {
            return;
        }
        self.ui.cards.post_upload(PendingUpload::Trip { id });
        self.sweep_cards();
    }

    /// A raised warning: accumulate the flags and deliver (or defer) the advisory card.
    pub(crate) fn on_warning(&mut self, flags: WarningFlags) {
        if flags.is_empty() {
            return;
        }
        self.ui.cards.post_warning(flags);
        self.sweep_cards();
    }

    /// The screen currently on top of the stack (receiving input). Always present — the Home root is
    /// never popped. A read-only handle for a host/test that needs to know which screen is up.
    /// The visible screen-stack depth — test/diagnostic observability (the WX11 alert tests pin
    /// "update in place, never stack" through it).
    pub fn debug_stack_len(&self) -> usize {
        self.ui.stack.len()
    }

    /// Offer one journaled ride recovered at boot to the rider.
    ///
    /// The host calls this after it has reconstructed `continuation` from the durable sample
    /// prefix. The first successful call restores the accumulators and roots the UI at the explicit
    /// Continue / hold-to-Discard card. Repeated calls are no-ops, so a level-style recorder status
    /// may be fed every pass without reopening the decision after it has been made.
    ///
    /// Returns `true` exactly when the card was raised. An already-tracking app refuses the offer;
    /// recovery is a boot decision, never something that can replace a live session.
    pub fn offer_recovered_ride(&mut self, continuation: crate::RideContinuation) -> bool {
        if !self.recorder.offer_recovery() {
            return false;
        }
        self.activity.restore_ride_continuation(continuation);
        self.activity.mode = Mode::Idle;
        self.activity.active_route = None;
        screen::apply(
            &mut self.ui.stack,
            screen::Transition::Root(Screen::RideRecovery(crate::screen::RideRecoveryScreen::new())),
        );
        self.ui.map_dirty = true;
        self.ui.input.cancel_holds();
        self.ui.hold_cancel_pending = true;
        true
    }

    /// Surface a durable recording whose journal bytes or continuation metadata failed domain
    /// validation. The fail-closed card has no Continue action; the rider may only hold-to-Discard,
    /// and Back cannot silently strand the object behind Home.
    pub fn offer_damaged_ride(&mut self) -> bool {
        if !self.recorder.offer_recovery() {
            return false;
        }
        self.activity.restore_ride_continuation(crate::RideContinuation::default());
        self.activity.mode = Mode::Idle;
        self.activity.active_route = None;
        screen::apply(
            &mut self.ui.stack,
            screen::Transition::Root(Screen::RideRecovery(crate::screen::RideRecoveryScreen::damaged())),
        );
        self.ui.map_dirty = true;
        self.ui.input.cancel_holds();
        self.ui.hold_cancel_pending = true;
        true
    }

    pub fn top_screen(&self) -> &Screen {
        self.ui.stack.last().expect("the stack always has the Home root")
    }

    /// Apply one device-wide [`Chord`] — **the drawer owner**, and the only place a drawer opens
    /// or closes. Returns whether it moved anything.
    ///
    /// Resolved here rather than in a screen because a chord is not a screen's input: the
    /// recogniser already swallowed its constituents, and the sheet has to be able to open over
    /// whatever the rider is on. Two rules live here and nowhere else — the **suppression set**
    /// (a genuinely blocking modal declares [`Caps::blocks_chords`](crate::screen::Caps) and no
    /// chord reaches past it) and **mutual exclusion** (one drawer at a time; the same chord again
    /// closes the one that is up).
    pub fn apply_chord(&mut self, chord: Chord) -> bool {
        if self.ui.stack.last().is_some_and(|s| s.caps().blocks_chords) {
            return false;
        }
        match chord {
            Chord::Quick => self.toggle_drawer(Screen::QuickDrawer(QuickDrawerScreen::new(self.ui.now_ms))),
            // D3 gives the contextual drawer its declarative per-screen content. Until then the
            // chord is recognised — so a Down+Back squeeze can never leak a step and a Back — and
            // deliberately does nothing on every screen.
            Chord::Context => false,
        }
    }

    /// Put `drawer` on the stack, taking off whatever drawer was already there. A repeat of the
    /// same drawer therefore toggles it shut, and the other one swaps in rather than stacking.
    fn toggle_drawer(&mut self, drawer: Screen) -> bool {
        let opening = drawer.row();
        let closed = match self.ui.stack.last() {
            Some(top) if top.is_overlay() => {
                let row = top.row();
                self.ui.stack.pop();
                Some(row)
            }
            _ => None,
        };
        if closed != Some(opening) {
            screen::apply(&mut self.ui.stack, screen::Transition::Push(drawer));
        }
        // The stack moved either way, so the frame is dirty and any hold charging underneath was
        // aimed at a screen the sheet has just covered (or uncovered) — the #480 rule, which a
        // chord earns exactly like a gesture. A chord is also user activity: the idle clock resets.
        self.ui.map_dirty = true;
        self.ui.last_input_ms = self.ui.now_ms;
        self.ui.idle_return_timing = true;
        self.ui.input.cancel_holds();
        self.ui.hold_cancel_pending = true;
        true
    }

    /// The brightness the panel should be driven at **this frame**: the quick drawer's staged
    /// preview while its editor is on top, and the committed
    /// [`Settings::brightness`](crate::Settings) row everywhere else.
    ///
    /// A host applies it through the [`Backlight`](obc_ports::Backlight) port. Because the answer
    /// is *derived* rather than latched, "Back cancels and reverts the preview" needs no undo
    /// path: the editor closes and the next frame reads the committed row again.
    ///
    /// **Only the top screen is asked**, exactly as [`power_off_requested`](App::power_off_requested)
    /// does. A host-pushed modal is pushed *above* the drawer, which stays on the stack mid-edit:
    /// scanning the whole stack would hold an uncommitted preview behind a card the rider cannot
    /// dismiss — for the length of a map transfer, whose own card also refuses the chord that would
    /// close the sheet. A preview belongs to a control the rider can see.
    pub fn backlight_level(&self) -> u8 {
        match self.ui.stack.last() {
            Some(Screen::QuickDrawer(d)) => d.staged_brightness(),
            _ => None,
        }
        .unwrap_or(self.settings.brightness)
        .min(crate::screen::BRIGHTNESS_MAX)
    }

    /// Whether the rider completed the quick drawer's guarded power-off hold. The host renders the
    /// powering-off frame this reports on, presents it, and then calls the
    /// [`PowerOff`](obc_ports::PowerOff) port — which does not return.
    pub fn power_off_requested(&self) -> bool {
        matches!(self.ui.stack.last(), Some(Screen::QuickDrawer(d)) if d.powering_off())
    }

    /// Number of POIs in the current [`poi_scratch`](App::poi_scratch) snapshot (0 when none has
    /// been taken). A test/introspection hook for the POIs browser's static snapshot.
    pub fn poi_snapshot_len(&self) -> usize {
        self.ui.poi_scratch.len()
    }

    /// Ask for a **route-corridor POI snapshot** (epic #946, U2): the map POIs of `filter` sitting
    /// within the corridor of the route ahead of `anchor_m`, frozen once taken. The query runs on
    /// the next rendered frame that carries both a map `Reader` and the streamed route — until then
    /// [`base_needs_reader`](App::base_needs_reader) keeps asking the host to build the `Reader`.
    ///
    /// Re-arming an unchanged `(filter, anchor_m)` is a no-op, so this is safe to call repeatedly;
    /// a changed filter (or a new anchor) drops the stale rows and re-queries.
    ///
    /// **Since U3 the request belongs to the screen stack**, not to this call: a screen declares the
    /// key it wants through [`Screen::corridor_request`](crate::screen::Screen) and
    /// [`reconcile_corridor`](crate::ui_runtime::UiRuntime::reconcile_corridor) re-points the scratch
    /// at it after every gesture and per-pass sweep — which is what disarms a request whose screen
    /// went away. So a request armed *here* survives only until the next reconcile unless some screen
    /// on the stack asks for the same key: this is the test/introspection door (and the pre-U3 seam),
    /// while a new consumer (U5's "Next: \<category\>" stat fields) adds its own `corridor_request`
    /// arm rather than calling this.
    pub fn arm_corridor(&mut self, filter: obc_reader::PoiCategorySet, anchor_m: u32) {
        self.ui.corridor_scratch.arm(crate::corridor::CorridorKey { filter, anchor_m });
    }

    /// Drop the held corridor snapshot **and** the request — the Up-ahead screen closing. The
    /// reader-build seam goes quiet again.
    pub fn clear_corridor(&mut self) {
        self.ui.corridor_scratch.disarm();
    }

    /// Drop the held corridor snapshot but keep the request armed, so the next frame with a
    /// `Reader` re-runs the identical query — the "re-enter refreshes" half of the frozen-snapshot
    /// contract (#115).
    pub fn invalidate_corridor(&mut self) {
        self.ui.corridor_scratch.invalidate();
    }

    /// The frozen corridor snapshot, ascending by along-route distance — empty until one has been
    /// taken (and for a genuinely empty corridor). Read-only: U3 draws rows off this, U5 picks the
    /// nearest entry per category.
    pub fn corridor_snapshot(&self) -> &[obc_reader::CorridorPoi] {
        self.ui.corridor_scratch.entries()
    }

    /// Number of entries in the current corridor snapshot (0 when none has been taken).
    pub fn corridor_snapshot_len(&self) -> usize {
        self.ui.corridor_scratch.len()
    }

    /// Whether a corridor snapshot is armed but not yet taken — the fact
    /// [`base_needs_reader`](App::base_needs_reader) folds in. A test/introspection hook.
    pub fn corridor_snapshot_pending(&self) -> bool {
        self.ui.corridor_scratch.pending()
    }

    /// Re-roll the Home screensaver's contour pattern to `seed`. The app does this itself when the
    /// stack returns to Home; this is the host-facing hook for previewing a specific pattern.
    pub fn reseed_home(&mut self, seed: u32) {
        if let Some(Screen::Home(home)) = self.ui.stack.first_mut() {
            home.reseed(seed);
        }
    }

    /// Seed the live settings from the host's persistent store at boot. The host calls this
    /// once after construction with [`SettingsStore::load`](obc_ports::SettingsStore::load)'s
    /// value (or [`Settings::default`] when nothing is stored); it leaves the dirty flag clear,
    /// so seeding the boot value never triggers a needless write-back.
    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
        // Stamp the wall clock to the persisted *local* set-point as of now (boot millis), so it
        // resumes from the stored time. `local_clock` folds the UTC offset out of the UTC anchor, so
        // the Home clock shows local time. This seed is display-only: `clock_trust` stays Untrusted
        // until a real source (GPS/BLE) re-stamps this boot.
        self.wall_clock.set(self.settings.local_clock(), self.ui.now_ms);
        // The value came from the store (or the default), so it is already persisted: reset the
        // revision handshake to Clean. Any pending edit is discarded — seeding is a boot/reload
        // operation, not a rider edit (the BLE-merge path uses `merge_ble_settings`, which preserves
        // a pending device-edit save).
        self.settings_ops.note_seeded();
    }

    /// Seed the weather **alert-mark record** from the host's durable storage at boot — the twin of
    /// [`set_settings`](App::set_settings) for the anchors, called once after construction.
    ///
    /// `provenance` is what decides whether the seed owes a write:
    ///
    /// - [`Record`](MarksProvenance::Record) — the marks came from the marks record (or there were
    ///   none). Already persisted, so the handshake resets to Clean.
    /// - [`LegacyBlob`](MarksProvenance::LegacyBlob) — the marks came out of the frozen v16 span of
    ///   a stored preferences blob. They are the rider's real anchors and nothing has written them
    ///   to the record yet, so this arms the handshake and the next pass rehomes them. Once a
    ///   record exists, the fallback can never fire again.
    pub fn set_alert_marks(&mut self, marks: crate::weather_alerts::AlertMarks, provenance: MarksProvenance) {
        self.weather.set_alert_marks(marks);
        match provenance {
            MarksProvenance::Record => self.alert_marks_ops.note_seeded(),
            MarksProvenance::LegacyBlob => self.alert_marks_ops.note_edited(),
        }
    }

    /// The weather domain, read-only — what the rider may be told about weather, in one place.
    /// Executors and tests observe through it; every *change* goes in as a fact, an intent or an
    /// outcome, which is why this hands out `&` and not `&mut`.
    pub fn weather(&self) -> &crate::weather::WeatherDomain {
        &self.weather
    }

    /// The live weather alert-mark anchors — what an executor writes when it serves
    /// [`SettingsEffect::PersistAlertMarks`](crate::settings::SettingsEffect).
    pub fn alert_marks(&self) -> &crate::weather_alerts::AlertMarks {
        self.weather.alert_marks()
    }

    /// Merge the BLE-owned fields (units + device name) of a phone Config write into the live
    /// settings, **preserving** any pending device-edit persistence (#456 + #810). The phone's write
    /// is persisted to the same store by the BLE plane directly (`ObjectStore::apply_config`), so this
    /// only reconciles the live RAM copy; it deliberately does **not** touch the revision handshake:
    ///
    /// - If a device edit was already pending, its revision is untouched, so its save still fires and
    ///   writes the merged blob — neither the phone's nor the rider's change is lost.
    /// - If nothing was pending, the live copy now matches what the BLE plane already persisted, so
    ///   staying Clean is correct (no redundant re-write).
    ///
    /// Only [`adopt_ble_fields`](crate::settings::Settings::adopt_ble_fields)'s narrow set is pulled
    /// across, so a device-only edit is never clobbered.
    pub fn merge_ble_settings(&mut self, other: &Settings) {
        self.settings.adopt_ble_fields(other);
    }

    /// The live device settings — read by the host to persist them, and by anything that needs
    /// the current units / clock / GPS-interval outside the screen draw path.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The live wall-clock time right now (see [`WallClock`]). What a screen draws as `HH:MM`;
    /// exposed for a host wanting the current time outside the draw path.
    pub fn wall_clock_now(&self) -> DateTime {
        self.wall_clock.now(self.ui.now_ms)
    }

    /// Whether the wall clock has an **established** set-point — a persisted/GPS/BLE time has been
    /// applied, versus a fresh clock that has never been told the time (see
    /// [`WallClock::is_established`](crate::wall_clock::WallClock::is_established)). The Home date
    /// line gates on this so it never shows a date with no origin at all. This is the *coarse*
    /// "do we know a date?" gate — a **stale persisted** set-point is established but **not**
    /// [`trusted`](App::clock_trusted); the auto-expiry sweep uses the finer trust gate.
    pub fn clock_is_set(&self) -> bool {
        self.wall_clock.is_established()
    }

    /// Whether the wall clock was established from a **real time source this boot** — GPS now, BLE in
    /// epic #638 S2 (see [`ClockTrust`]). `false` from every boot until the first
    /// [`stamp_clock`](App::stamp_clock), regardless of any stale persisted set-point. S3's expiry
    /// sweep gates every timestamp write and deletion on this: no trusted clock → nothing is stamped
    /// or deleted.
    pub fn clock_trusted(&self) -> bool {
        self.clock_trust != ClockTrust::Untrusted
    }

    /// The single entry point that establishes a **trusted** wall clock from a real time source —
    /// GPS (`tick`) and BLE `setClock` ([`stamp_clock_ble`](App::stamp_clock_ble), epic #638 S2).
    /// Both funnel here so one place owns the invariant. It: optionally sets the persisted UTC
    /// `offset` (BLE passes `Some`, GPS `None`); sets the persisted UTC `clock` anchor to `utc`;
    /// re-stamps the live [`WallClock`] against the map-plane clock (`now_ms`), **back-dating the
    /// epoch by `second`** (the fix's seconds-into-the-minute) so the displayed minute rolls at the
    /// true instant, not up to a fix-interval late; persists the new set-point through the
    /// change-detected settings-save path (armed only on the first trusted stamp of the boot or a
    /// real offset change, so a per-fix GPS stamp never thrashes RRAM); and records the trust
    /// `source`.
    pub fn stamp_clock(&mut self, utc: DateTime, second: u8, offset: Option<i16>, source: ClockTrust) {
        // Persist the set-point only when it's worth an RRAM write. The persisted `clock` exists
        // solely to seed the *boot* display clock — which is untrusted until the next boot's first
        // stamp re-establishes it within seconds — so a mid-ride re-stamp buys only display-only,
        // untrusted precision nobody sees, at the cost of a store write (+ #810 revision bump) on
        // every displayed-minute roll for a whole ride. So arm a save only on the untrusted→trusted
        // transition (the first trusted stamp of the boot) or when the persisted UTC offset actually
        // moves. GPS carries no offset (`None` — leave it untouched); BLE `setClock` carries the
        // phone's live offset (`Some`) and MUST persist a change even on a same-boot reconnect (DST /
        // travel), so the offset is applied **here, before** the change-check below fires the guard —
        // setting `settings.utc_offset_min` in the BLE handler *before* calling this would hide the
        // change from `offset_before` and drop the save. Either way the live `WallClock` re-stamps
        // below on *every* stamp, so the displayed time stays exact.
        let first_trusted_this_boot = self.clock_trust == ClockTrust::Untrusted;
        let offset_before = self.settings.utc_offset_min;
        if let Some(offset) = offset {
            self.settings.utc_offset_min = offset;
        }
        self.settings.clock = utc;
        let epoch = self.ui.now_ms.wrapping_sub(second as u32 * 1000);
        self.wall_clock.set(self.settings.local_clock(), epoch);
        if first_trusted_this_boot || self.settings.utc_offset_min != offset_before {
            self.settings_ops.note_edited();
        }
        self.clock_trust = source;
    }

    /// Stamp the wall clock from a BLE `setClock` (auto-expiry epic #638 S2, #642): the phone's UTC
    /// **unix seconds** + its live local offset, arriving over the encrypted link on every connect.
    /// The board crate's BLE plane validates the wire (spec §4.4) and hands the two decoded values
    /// straight here, so the unix→`DateTime` split (and the seconds-into-the-minute back-date) stays
    /// in `obc-app` beside [`stamp_clock`](App::stamp_clock), the one owner of that arithmetic — the
    /// GPS path already carries a split `DateTime`+`second`, so only BLE needs the conversion.
    ///
    /// Passes the offset as `Some`, so a changed offset persists even when the clock is already
    /// trusted this boot (a same-boot reconnect after a flight); records trust as
    /// [`Ble`](ClockTrust::Ble).
    pub fn stamp_clock_ble(&mut self, utc_unix: u32, offset_min: i16) {
        let utc = DateTime::from_unix(utc_unix);
        let second = (utc_unix % 60) as u8;
        self.stamp_clock(utc, second, Some(offset_min), ClockTrust::Ble);
    }

    /// The current **UTC** unix seconds, from the wall clock. The clock's set-point is local time
    /// (the UTC anchor shifted by the offset), so the persisted UTC offset is folded back out.
    pub fn wall_unix_now(&self) -> u32 {
        let local = self.wall_clock.unix_now(self.ui.now_ms);
        (local as i64 - self.settings.utc_offset_min as i64 * 60) as u32
    }

    /// The app-side half of the §11.4 weather request context (WX8, #1193), distilled to the
    /// [`WeatherSnapshot`](crate::ble::WeatherSnapshot) the host's weather plane reads each pass —
    /// the reverse direction of [`set_ble_status`](App::set_ble_status), and like it free of any
    /// wire type.
    ///
    /// Honesty rules (the spec's flags-not-sentinels discipline):
    /// - **position** is served only while the last fix is *fresh* (≤ [`WEATHER_FIX_FRESH_MS`])
    ///   **and** the wall clock was established from a real source this boot — a fix the app can't
    ///   date has no `fix_utc` to give, and the spec guards all three fields with one bit. The
    ///   fix's UTC is the wall clock read back by the fix's age, exact to the second at the 1 Hz
    ///   cadence the receiver runs at.
    /// - **bearing** is the GPS course only while actually *moving* (≥ 1 m/s): a stationary
    ///   receiver's course is noise, not a travel bearing the device believes. The compass is
    ///   deliberately not substituted — it says where the *device* points, not where the rider
    ///   travels.
    /// - **route id** is the active route's durable object id — the id the phone's route list
    ///   already knows — and absent for a route that has none resident.
    pub fn weather_snapshot(&self) -> crate::ble::WeatherSnapshot {
        let now_utc = if self.clock_trusted() { Some(self.wall_unix_now()) } else { None };
        // The fresh fix + its age on the map-plane clock (the same timebase `last_fix_ms` stamps).
        let fresh = match (self.state.user_fix, self.ride.last_fix_ms) {
            (Some(fix), Some(at_ms)) => {
                let age_ms = self.ui.now_ms.wrapping_sub(at_ms);
                if age_ms <= WEATHER_FIX_FRESH_MS {
                    Some((fix, age_ms))
                } else {
                    None
                }
            }
            _ => None,
        };
        let position = match (fresh, now_utc) {
            (Some((fix, age_ms)), Some(now)) => Some(crate::ble::WeatherFix {
                lat_udeg: fix.lat,
                lon_udeg: fix.lon,
                fix_utc: now as i64 - (age_ms / 1000) as i64,
            }),
            _ => None,
        };
        let speed_mps = fresh.and_then(|(fix, _)| fix.speed_mps);
        let moving = speed_mps.is_some_and(|s| s >= 1.0);
        let bearing_deg = if moving {
            fresh.and_then(|(fix, _)| fix.course).map(|c| {
                // Wrap into `0..360` with core-only float ops (`rem_euclid` needs libm here).
                let mut deg = c % 360.0;
                if deg < 0.0 {
                    deg += 360.0;
                }
                deg as u16 % 360
            })
        } else {
            None
        };
        let speed_deci_ms = speed_mps.map(|s| (s.max(0.0) * 10.0).min(u16::MAX as f32) as u16);
        crate::ble::WeatherSnapshot {
            ride_active: self.recorder.recording(),
            position,
            bearing_deg,
            speed_deci_ms,
            // The weather request's legacy compact route id remains optional until that protocol
            // moves to the flat store's full-width ObjectId. Never truncate a valid catalog id.
            route_id: self
                .activity
                .active_route
                .and_then(|i| self.catalogs.route_id_at(i))
                .and_then(|id| u16::try_from(id).ok()),
            now_utc,
        }
    }

    /// The ride totals + wall-clock anchor for the ride object's footer, read by the executor as it
    /// performs [`RecorderEffect::Finalize`](crate::recorder::RecorderEffect) so the anchor pairs
    /// with the log's last points.
    pub fn ride_stats(&self) -> obc_route::RideStats {
        obc_route::RideStats {
            distance_m: self.activity.ridden_m as u32, // float→int casts saturate
            moving_time_s: self.activity.moving_s as u32,
            avg_speed_cms: self.activity.avg_speed_cms(),
            climb_m: self.activity.climb_m() as u16,
            unix_at_anchor: self.wall_unix_now(),
            anchor_ms: self.ui.now_ms,
            clock_trusted: self.clock_trusted(),
            // The per-ride BLE-sensor summary is captured in the v3 footer (epic #707, SE3). Each is
            // `None` (→ sentinel) when the ride saw no fresh sample of that quantity.
            avg_hr: self.activity.avg_hr(),
            max_hr: self.activity.max_hr(),
            avg_cadence: self.activity.avg_cadence(),
            avg_power: self.activity.avg_power(),
            max_power: self.activity.max_power(),
        }
    }

    /// Whether this device can record a ride at all — [`Capabilities::recorder`], the level stage 12
    /// calculated last pass.
    ///
    /// A **host** reads it for the same reason a screen does: to not ask for something the device
    /// cannot do. A host that asks anyway is told, through the recording warning, and the request is
    /// kept — but a page or a tour that opens a ride *for* the rider should wait for the device to
    /// report its card rather than put a card on glass at boot.
    pub fn can_record(&self) -> bool {
        self.pass.capabilities.recorder.record
    }

    /// Whether a ride is open — recording, paused, or closing. The one read of Recorder's session
    /// state a host or a suite needs; nothing else keeps a copy of it.
    pub fn recording(&self) -> bool {
        self.recorder.recording()
    }

    /// The open ride's session id, or `None` — the level an executor keys its ride log on. A change
    /// means "open a new log".
    pub fn ride_session(&self) -> Option<u32> {
        self.recorder.session()
    }

    /// Test hook: arm a pending settings save without driving a real edit (bumps the revision and
    /// marks Dirty), standing in for a settings-screen edit the drain/gating tests don't replay.
    #[cfg(test)]
    fn arm_settings_save(&mut self) {
        self.settings_ops.arm_save();
    }

    /// Whether the top screen would draw a live **hold fill** for its current selection/state —
    /// a guarded confirm row (Ride control, Route swap), the armed factory-Reset bar, or the
    /// Fields hold-to-delete footer over a deletable row. A render-on-demand host combines this
    /// with the charging hold-progress to redraw only when the fill would actually animate;
    /// holding Select on any other screen changes no pixels, so no repaint is owed.
    pub fn top_wants_hold_fill(&self) -> bool {
        self.ui.stack.last().is_some_and(|s| {
            s.wants_hold_fill(
                &self.settings,
                &self.state,
                &self.activity,
                self.recorder.recording(),
                self.catalogs.routes(),
                self.catalogs.rides(),
            )
        })
    }

    /// **Debug/benchmark hook** (the USB-CDC `Z` command): set the map camera to exactly `mpp`
    /// meters-per-pixel and force one map redraw. Drives the zoom directly (bypassing Select's
    /// fixed steps) so a render sweep can pin an exact scale per sample. Part of the strippable
    /// render-instrumentation seam.
    pub fn set_map_mpp(&mut self, mpp: f32) {
        self.state.zoom = zoom_for_mpp(mpp);
        self.ui.map_dirty = true;
    }

    /// Recognise this frame's raw control input and apply each resulting gesture to the top screen,
    /// then advance the visible screens' timed content. Fuses the two planes into one call for the
    /// single-loop hosts (the simulator, the web demos); `clock` is the [`InputClock`] for hold timing.
    /// Call once per frame even with no pending events — that is how a held button's long-press
    /// fires.
    ///
    /// The two-plane firmware does **not** call this: its high-priority plane recognises gestures
    /// and feeds them back through [`apply_gesture`](App::apply_gesture), while
    /// [`advance_animations`](App::advance_animations) runs on the map plane. This is exactly those
    /// two halves over `App`'s own [`InputPlane`].
    pub fn handle_input(&mut self, clock: InputClock, input: &mut dyn InputSource) {
        self.ui.now_ms = clock.0;
        // The borrow split is the point: `recognize` borrows `self.ui.input`, so gestures are buffered
        // there and applied *after* it returns (`apply_gesture` touches other fields, never
        // `self.ui.input`). Recognition depends only on the raw events + clock, so this is identical to
        // applying inline; the buffer capacity dwarfs one frame's bounded events.
        let mut pending: heapless::Vec<Gesture, GESTURE_BUF> = heapless::Vec::new();
        let chord = self.ui.input.recognize(clock, input, |g| {
            let _ = pending.push(g);
        });
        // Above the screen: the chord resolves first, so this frame's gestures (which by
        // construction are not its constituents) land on whatever the drawer left on top.
        if let Some(chord) = chord {
            self.apply_chord(chord);
        }
        self.apply_gesture_batch(&pending);
        // A single-loop host has no second recognizer to cancel, so it consumes the latch the batch
        // may have set rather than leaving it for a plane that does not exist.
        let _ = self.take_hold_cancel();
        self.advance_animations(clock);
    }

    /// Recognise this frame's raw input into gestures **without applying them** — the recognition
    /// half of [`handle_input`](App::handle_input), for a single-loop host that drives
    /// [`run_pass`](App::run_pass) and hands the batch in as
    /// [`PassInputs::gestures`](crate::device_core::PassInputs).
    ///
    /// The clock is the recognizer's alone: the map plane's own `now_ms` is the pass's to set, at
    /// its input stage, from the same frame's clock.
    pub fn recognize(&mut self, clock: InputClock, input: &mut dyn InputSource) -> heapless::Vec<Gesture, GESTURE_BUF> {
        let mut pending: heapless::Vec<Gesture, GESTURE_BUF> = heapless::Vec::new();
        let chord = self.ui.input.recognize(clock, input, |g| {
            let _ = pending.push(g);
        });
        // A chord is not a gesture and never reaches the pass's gesture batch: it is resolved here,
        // above the screen stack, exactly as `handle_input` resolves it.
        if let Some(chord) = chord {
            self.apply_chord(chord);
        }
        pending
    }

    /// Apply one frame's recognised gestures **in order**, dropping a `Hold`/`BackHold` that was
    /// already recognised into the batch behind a gesture that changed the screen stack.
    ///
    /// That drop is issue #480: the transition cancels any hold still charging on the recognizer,
    /// but a completed hold sitting in this batch escaped it — it was aimed at the old top (a
    /// popup's "Save & new"), and completing it onto the new one can be destructive (the Route
    /// menu's hold-to-delete footer). The board applies the same rule around its own gesture channel
    /// because it must also cancel its second input plane; this is the one place a host with a
    /// single plane needs.
    pub(crate) fn apply_gesture_batch(&mut self, gestures: &[Gesture]) {
        let mut cancelled = false;
        for &g in gestures {
            if cancelled && matches!(g, Gesture::Hold | Gesture::BackHold) {
                continue;
            }
            cancelled |= self.apply_gesture_reporting_stack_change(g);
        }
    }

    /// Drain the pending hold-cancel edge (see `hold_cancel_pending`): `true` when a gesture
    /// changed the screen stack since the last drain, i.e. any hold charging on the host's input
    /// plane is aimed at a vanished target and must be cancelled
    /// ([`InputPlane::cancel_holds`](crate::InputPlane::cancel_holds)). The two-plane firmware
    /// checks this after each drained gesture; [`handle_input`](App::handle_input) consumes it
    /// itself, so single-loop hosts never see it.
    pub fn take_hold_cancel(&mut self) -> bool {
        self.ui.take_hold_cancel()
    }

    /// Apply one recognised gesture to the top screen and run the navigation transition it returns —
    /// the **map plane's** half of input handling, split out from recognition. The two-plane
    /// firmware calls this per gesture from its high-priority plane's channel, so the transition
    /// lands a frame after the overlay confirmed the press. Uses the map plane's clock
    /// ([`now_ms`](App::now_ms)) for the [`Ctx`](screen::Ctx).
    pub fn apply_gesture(&mut self, g: Gesture) {
        let _ = self.apply_gesture_reporting_stack_change(g);
    }

    /// [`apply_gesture`](App::apply_gesture), reporting whether the transition **changed the screen
    /// stack** — the fact [`apply_gesture_batch`](App::apply_gesture_batch) needs to apply #480's
    /// drop rule without consuming the hold-cancel latch a second input plane still owns.
    fn apply_gesture_reporting_stack_change(&mut self, g: Gesture) -> bool {
        // Every screen renders into the map plane, so an applied gesture dirties it. Conservative by
        // design (a gesture a screen ignores still costs one redraw), which keeps the idle path
        // exact: with no gesture recognized, `apply_gesture` never runs and the map stays clean.
        self.ui.map_dirty = true;
        // Any recognised gesture is user activity: reset the idle-return clock (see
        // `apply_idle_return`). A gesture the screen ignores still counts — a step on Home, say.
        self.ui.last_input_ms = self.ui.now_ms;
        self.ui.idle_return_timing = true;
        // Snapshot the settings so a settings-screen edit is detected by one `==` (Settings is
        // `Copy + Eq`). A change flags a save for the host to pick up via `take_settings_dirty`.
        let settings_before = self.settings;
        // Navigator's detour level before the screen speaks, so a cancellation it admits takes the
        // preview polyline with it (see `sync_detour_preview`).
        let detour_planned_before = self.navigator.detour_planned();
        let backlight_available = self.backlight_available;
        let App {
            state,
            activity,
            settings,
            catalogs,
            nav_profiles,
            ride,
            recorder,
            ui,
            navigator,
            dfu,
            storage,
            weather,
            ..
        } = self;
        let mut cx = Ctx {
            state,
            activity,
            settings,
            navigator,
            recorder,
            dfu,
            storage,
            weather,
            routes: catalogs.routes(),
            rides: catalogs.rides(),
            trips: catalogs.trips(),
            nav_profiles,
            backlight: backlight_available,
            poi_scratch: &ui.poi_scratch,
            // The Up-ahead timeline's two source tables, read-only: `handle` must see exactly the
            // merged rows `draw` drew, or a Press would open the wrong row (epic #946, U3).
            waypoints: ride.waypoints.as_slice(),
            corridor: ui.corridor_scratch.entries(),
            sensor_scan_hits: ui.sensor_scan_hits.as_slice(),
            now_ms: ui.now_ms,
        };
        let t = ui.stack.last_mut().expect("the stack always has the Home root").handle(g, &mut cx);
        let depth_before = ui.stack.len();
        // Whether this transition actually changes the stack (Pop/Home at the root are no-ops).
        // A change invalidates any in-flight hold's target — see `hold_cancel_pending`.
        let stack_changed = match &t {
            screen::Transition::None => false,
            screen::Transition::Pop | screen::Transition::Home => depth_before > 1,
            screen::Transition::Push(_) | screen::Transition::Replace(_) | screen::Transition::Root(_) => true,
        };
        // The rider's Start becomes a session here rather than at stage 7, so the rest of this
        // gesture batch sees the ride the first of them opened. Same entry point either way.
        self.advance_recorder_session();
        self.sync_detour_preview(detour_planned_before);
        screen::apply(&mut self.ui.stack, t);
        // Opening a POI list drops any previous snapshot so its first draw re-queries at the current
        // fix — the "re-enter to refresh" contract (issue #425). Gated on this being a fresh open
        // (the stack grew), so a step *within* the list doesn't wipe the frozen snapshot.
        if self.ui.stack.len() > depth_before && matches!(self.ui.stack.last(), Some(Screen::PoiList(_))) {
            self.ui.poi_scratch.invalidate();
        }
        // The corridor snapshot follows the stack, not a gesture: whatever Up-ahead screen is on it
        // declares the `(filter, anchor)` it wants and this arms it — a fresh open additionally
        // re-takes the identical key, the "re-enter refreshes" half of the frozen-snapshot contract
        // (epic #946, U2/U3). Nothing on the stack wants one ⇒ the request is dropped and the
        // reader-build seam goes quiet.
        let fresh = self.ui.stack.len() > depth_before;
        self.ui.reconcile_corridor(fresh);
        // Returning to the bare Home root re-opens the screensaver — re-roll its contour seed so the
        // topo peaks drift for this visit. Gated on the *edge* (was deeper, now 1) so it fires once
        // per return; being in `apply_gesture` means a clock/battery re-render (which never touches
        // the stack) leaves the pattern put.
        if self.ui.stack.len() == 1 && depth_before > 1 {
            if let Some(Screen::Home(home)) = self.ui.stack.first_mut() {
                home.reseed(self.ui.now_ms);
            }
        }
        // The top screen changed under the rider's finger: cancel any hold charging right now
        // (both `App`'s own recogniser and, via the pending flag, the two-plane firmware's input
        // plane), so a long-press aimed at the *old* top can't complete onto the new one.
        if stack_changed {
            self.ui.input.cancel_holds();
            self.ui.hold_cancel_pending = true;
        }
        if self.settings != settings_before {
            // A rider edit: bump the revision and (re-)arm the save — superseding any in-flight or
            // backing-off older revision (#810); see `SettingsMachine::note_edited`.
            self.settings_ops.note_edited();
            // A change to the *local* set-point re-stamps the wall clock so Home shows the new local
            // time: the only settings-screen edit that shifts it now is a UTC-offset step (manual
            // date/time editing was removed in #641). It does **not** touch `clock_trust` — nudging
            // the offset isn't a real time source. Flipping units or the GPS interval leaves the
            // local clock alone.
            let local_now = self.settings.local_clock();
            if local_now != settings_before.local_clock() {
                self.wall_clock.set(local_now, self.ui.now_ms);
            }
        }
        stack_changed
    }

    /// Advance the **map plane's** clock to `clock` and poll each visible screen's timers
    /// ([`Screen::tick_timers`]) in one pass: any time-driven repaint that fired (the Statistics
    /// page flip, the Home clock's minute rollover) dirties the map — so a screen
    /// surfaces its own timed-refresh rather than the host re-rendering on a blind heartbeat — and
    /// the soonest residual deadline is stored for [`ms_until_next_wake`](App::ms_until_next_wake).
    /// Cheap: a clock comparison per drawn screen, over the same `base..` range
    /// [`render_map`](App::render_map) draws.
    ///
    /// [`handle_input`](App::handle_input) calls this for the single-loop hosts; the two-plane
    /// firmware calls it directly on its map plane.
    pub fn advance_animations(&mut self, clock: InputClock) {
        let now = self.wall_clock.now(clock.0);
        let ms_to_next_minute = self.wall_clock.ms_to_next_minute(clock.0);
        let pan_active = self.state.pan.is_some();
        let tracking = self.recorder.recording();
        // The timer poll itself — and every stack/dirty/wake mutation it makes — is the UI
        // runtime's; this method sequences the per-pass sweeps around it with the cross-component
        // facts they need.
        self.ui.advance_timers(clock.0, now, ms_to_next_minute, &self.settings, pan_active, tracking);
        // The one host-pushed-card sweep (epic #1397, S1): land anything a hold or a higher-ranked
        // card deferred on an earlier pass, and run the upload family's 30 s auto-close. Here — the
        // one hook every host runs each pass — rather than a new timer path; the popups'
        // `tick_timers` above already armed the wake that gets a parked device to this line at the
        // deadline. Before the idle sweep, so a card that lands this pass is on top when the sweep
        // checks its exemptions — an unacknowledged card must not be yanked Home by the idle return.
        self.sweep_cards();
        // The idle-return sweep (fire the return if we're past the deadline) and its residual wake,
        // folded into the deadline the event-driven host arms so a parked device wakes to return.
        self.ui.apply_idle_return(&self.settings, tracking);
        if let Some(rem) = self.ui.idle_return_remaining_ms(&self.settings, tracking) {
            self.ui.next_wake_ms = Some(self.ui.next_wake_ms.map_or(rem, |w| w.min(rem)));
        }
        // Every sweep above can move the stack (a popup lands, the idle return fires), so re-point
        // the corridor snapshot at what the stack now wants — a request left armed after the
        // Up-ahead list was swept away would keep the board building the map `Reader` forever
        // (epic #946, U3). Never a *fresh* open: only a gesture opens a screen.
        //
        // This also re-runs the `Next: <category>` tiles' refresh policy (U5) — the per-pass
        // decision of whether the cache wants one more single-category snapshot — and ends in the
        // same `reconcile_corridor`, so the stack still has the last word on the shared scratch.
        self.ui.reconcile_next_ahead(&self.settings, self.activity.active_route, self.activity.progress_m);
    }

    /// The single "next wake deadline" the event-driven host arms one timer to: the soonest, in
    /// millis from `now_ms`, that any visible screen needs a *timed* redraw — or `None` when nothing
    /// is time-animating (sleep until an input or sensor event). A read of the deadline
    /// [`advance_animations`](App::advance_animations) stored, so **call it right after
    /// `advance_animations`** in the same frame, with the same `now_ms` (debug-asserted): any *due*
    /// animation has then already fired, so the deadline is strictly in the future.
    pub fn ms_until_next_wake(&self, now_ms: u32) -> Option<u32> {
        debug_assert_eq!(
            now_ms, self.ui.now_ms,
            "ms_until_next_wake must follow advance_animations in the same frame, with the same now_ms"
        );
        self.ui.next_wake_ms
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
    ///
    /// `scratch` is the caller's [`RenderScratch`] — the render path's per-frame working memory,
    /// owned by the host rather than by `App` (#1146), lent for the duration of the call and
    /// meaningless between frames. It is optional because only the map-drawing screens ever touch
    /// it (#1146 P2): a host whose frame is pure chrome passes `None` and keeps its scratch memory
    /// for something else. `None` under a map-drawing base is a caller bug — the map is skipped and
    /// a `debug_assert!` fires.
    #[allow(clippy::too_many_arguments)]
    pub fn render_frame<D, F>(
        &mut self,
        scratch: Option<&mut RenderScratch>,
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
        let stats = self.render_map(scratch, target, reader, route, w, h, &color_fn);
        self.render_overlay(target, w, h, &color_fn);
        stats
    }

    /// [`render_frame`](App::render_frame) plus the optional **rain overlay lease** (WX10) — the
    /// frame-level entry a host with a mounted weather store uses; `None` is byte-identical to
    /// [`render_frame`](App::render_frame).
    #[allow(clippy::too_many_arguments)]
    pub fn render_frame_with_rain<D, F>(
        &mut self,
        scratch: Option<&mut RenderScratch>,
        target: &mut D,
        reader: &Reader,
        route: Option<&RouteReader>,
        rain: Option<&mut dyn obc_render::RainOverlaySource>,
        weather: Option<&crate::weather::WeatherSnapshot>,
        w: f32,
        h: f32,
        color_fn: F,
    ) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let stats = self.render_scene_map_rain_timed(
            scratch,
            target,
            Some(reader),
            Some(reader),
            route,
            rain,
            weather,
            w,
            h,
            &color_fn,
            &NoopClock,
        );
        self.render_overlay(target, w, h, &color_fn);
        stats
    }

    /// Render **only the map plane** — the screen stack from the topmost opaque screen upward, but
    /// **excluding** the global hold-hint chrome. Returns the map [`RenderStats`].
    ///
    /// The expensive half (24–51 ms on the device); a host that keeps the overlay on its own buffer
    /// renders this only when the map changed, then repaints the cheap
    /// [`render_overlay`](App::render_overlay) over it at a higher rate.
    #[allow(clippy::too_many_arguments)]
    pub fn render_map<D, F>(
        &mut self,
        scratch: Option<&mut RenderScratch>,
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
        self.render_scene_map_timed(scratch, target, Some(reader), Some(reader), route, w, h, color_fn, &NoopClock)
    }

    /// Like [`render_map`](App::render_map) but threads `clock` to the Map screen's
    /// [`render_timed`](obc_render::RenderScratch::render_timed), so the returned [`RenderStats`]
    /// carries the map's per-stage timings. The device's render benchmark uses this with its own
    /// microsecond clock. Part of the strippable render-instrumentation seam.
    #[allow(clippy::too_many_arguments)]
    pub fn render_map_timed<D, F>(
        &mut self,
        scratch: Option<&mut RenderScratch>,
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
        self.render_scene_map_timed(scratch, target, reader, reader, route, w, h, color_fn, clock)
    }

    /// Timed single-map render with the weather feed/rain lease used by the board. This is the
    /// weather-aware twin of [`render_map_timed`](App::render_map_timed); keeping the wrapper here
    /// avoids making a host name `Reader` as the generic scene type just to pass an optional map.
    #[allow(clippy::too_many_arguments)]
    pub fn render_map_rain_timed<D, F>(
        &mut self,
        scratch: Option<&mut RenderScratch>,
        target: &mut D,
        reader: Option<&Reader>,
        route: Option<&RouteReader>,
        rain: Option<&mut dyn obc_render::RainOverlaySource>,
        weather: Option<&crate::weather::WeatherSnapshot>,
        w: f32,
        h: f32,
        color_fn: F,
        clock: &dyn Clock,
    ) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        self.render_scene_map_rain_timed(scratch, target, reader, reader, route, rain, weather, w, h, color_fn, clock)
    }

    /// Generic timed map-plane render. `scene` drives geometry through [`MapScene`];
    /// `core_reader` drives the core-only POI/hours preparation. They are independently optional
    /// so chrome-only frames can skip every map source.
    #[allow(clippy::too_many_arguments)]
    fn render_scene_map_timed<D, F, S>(
        &mut self,
        scratch: Option<&mut RenderScratch>,
        target: &mut D,
        scene: Option<&S>,
        core_reader: Option<&Reader>,
        route: Option<&RouteReader>,
        w: f32,
        h: f32,
        color_fn: F,
        clock: &dyn Clock,
    ) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: MapScene,
    {
        self.render_scene_map_rain_timed(scratch, target, scene, core_reader, route, None, None, w, h, color_fn, clock)
    }

    /// Generic timed scene-map rendering plus the optional **rain overlay lease** (WX10): a host
    /// that mounted a weather store passes the frame's
    /// [`RainOverlayAdapter`](crate::RainOverlayAdapter) (or any [`RainOverlaySource`]) every
    /// frame and the base screen renders precipitation below the road band **if its
    /// [`Caps::rain_overlay`](crate::screen::Caps::rain_overlay) says it wants it** — today only
    /// the WX11 rain map. On any other screen the lease is dropped here, so a mounted weather store
    /// never tints the ordinary Map; `None` is byte-identical to the plain call. This is the single
    /// production hook — firmware and simulator both land here.
    #[allow(clippy::too_many_arguments)]
    pub fn render_scene_map_rain_timed<D, F, S>(
        &mut self,
        scratch: Option<&mut RenderScratch>,
        target: &mut D,
        scene: Option<&S>,
        core_reader: Option<&Reader>,
        route: Option<&RouteReader>,
        rain: Option<&mut dyn obc_render::RainOverlaySource>,
        weather: Option<&crate::weather::WeatherSnapshot>,
        w: f32,
        h: f32,
        color_fn: F,
        clock: &dyn Clock,
    ) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: MapScene,
    {
        // Record the panel size for the screen ticks' region reporting (`advance_animations`) —
        // the one place every host states its real frame dimensions.
        self.ui.frame_size = (w as i16, h as i16);
        // Route-relative pan steps are recorded by gesture handling as a cumulative-distance
        // cursor because `Ctx` deliberately owns no streamed reader. Resolve that cursor here,
        // once per dirty step, before `Render` borrows state read-only for the draw pass.
        if let Some(route) = route {
            self.state.sync_pan_route(route);
        }
        // Drain the one-shot region clip (see `set_render_clip`) — `None` on every normal frame.
        let render_clip = self.ui.render_clip.take();

        // Rebuild the cached elevation profile when the active route changes — it streams every
        // chunk, so it's built once on load, never per frame; clears when no route is loaded.
        self.ride.refresh_route_profile(self.activity.active_route, route);
        // Invalidate the resident **ride** profile + track preview the moment they stop matching
        // the viewed ride (#680; the preview joined in #678 rework 3): the detail exited
        // (`viewed_ride` cleared) or moved subjects. Filling is the executor's keyed answer; only
        // the drop lives here, so a stale band/shape is never drawn.
        let key = self.catalogs.ride_track_key(self.activity.viewed_ride);
        self.catalogs.drop_stale_ride_views(key);

        // Pre-draw acquisition (#803): the base screen resolves any streamed-reader state (POI
        // snapshot / hours or Detour route geometry) before the draw loop, so `Render` carries
        // the POI scratch read-only and every screen's `draw` is side-effect-free.
        self.ui.prepare_base(
            core_reader,
            route,
            self.state.user_fix,
            self.activity.active_route,
            self.activity.progress_m,
            self.activity.route_total_m,
            self.catalogs.detour_preview_for(self.activity.active_route),
        );

        // Computed before the field borrow below splits `self`.
        let now = self.wall_clock.now(self.ui.now_ms);
        let clock_set = self.wall_clock.is_established();
        // The UTC instant the Route overview's expiry row counts down from. Display-only, so
        // (unlike the sweep) it isn't gated on the clock being trusted — a stale set-point just
        // yields a stale readout.
        let now_utc = self.wall_unix_now();
        let base = self.ui.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        // The rain overlay is the **base screen's** declared capability
        // ([`Caps::rain_overlay`](crate::screen::Caps::rain_overlay)), not the host's: a host mounts
        // a weather store once and then leases a frame unconditionally, so this is the one place
        // that decides whether the frame's rain may be drawn at all. Dropped here — before any
        // screen can `take` it — the ordinary Map, the Detour pair, and every map base added later
        // are rain-free by construction rather than by an exit hook a screen transition could
        // forget. It also keeps the *per-tile* decodes off every frame no screen would have painted
        // rain on — which will matter once the board renders rain (today only `obc-sim` leases;
        // `obc-fw-nrf54l` still renders through `render_map_timed` and builds no adapter). Note it
        // is only the `tile` reads that are skipped: the adapter's own header/frame reads happen in
        // `RainOverlayAdapter::at_step`, upstream of this gate.
        let rain = if self.ui.base_wants_rain() { rain } else { None };
        // The in-screen confirm fill's hold-progress. Prefer a host-supplied value (the two-plane
        // firmware's separate input plane); fall back to `App`'s own input on the single-loop hosts.
        let hold_progress = self.ui.hold_progress_override.unwrap_or_else(|| self.ui.input.select_hold_progress());
        let no_fix = !self.has_live_fix(self.ui.now_ms);
        let backlight_available = self.backlight_available;
        // The one cue the weather screens raise over cached content, read from its owner — the same
        // shape as `card_free_bytes: storage.free_bytes()` below. A `refreshing` bool crossing a
        // render signature is what let the platform's copy and the domain's answer disagree.
        let weather_refreshing = self.weather.refreshing();
        let App {
            state,
            activity,
            settings,
            catalogs,
            ride,
            recorder,
            ui,
            nav_profiles,
            fw_version,
            map_name,
            map_obcm_version,
            storage,
            ..
        } = self;
        // The shape previews draw only for the subject they were decimated for — a stale key
        // (route/ride changed, preview not re-fed yet) hands the screens an empty slice.
        let nav_key = catalogs.nav_preview_key(activity.active_route);
        let ride_key = catalogs.ride_track_key(activity.viewed_ride);
        let nav_preview: &[(i32, i32)] = catalogs.nav_preview_for(nav_key);
        let ride_preview: &[(i32, i32)] = catalogs.ride_preview_for(ride_key);
        let detour_preview: &[(i32, i32)] = catalogs.detour_preview_for(activity.active_route);
        // Bundle the active climb for the screens: the resident detail buffer is only meaningful
        // when a climb is active, so hand out the `(seg, profile)` pair exactly when `active_climb`
        // resolves to a live segment — a stale buffer is never reachable through `Render`.
        let climb = activity
            .active_climb
            .and_then(|i| ride.climbs.as_slice().get(i))
            .map(|seg| screen::ActiveClimb { seg, profile: &ride.climb_profile });
        let rx = Render {
            scratch,
            // Reborrow so the lease's trait-object lifetime shrinks to this frame's `Render`
            // borrow (a `&mut dyn` is invariant without the explicit coercion).
            rain: rain.map(|r| &mut *r as &mut dyn obc_render::RainOverlaySource),
            state,
            activity,
            settings,
            routes: catalogs.routes(),
            route_metas: catalogs.route_metas(),
            rides: catalogs.rides(),
            trips: catalogs.trips(),
            nav_profiles,
            route,
            profile: ride.profile.as_ref(),
            ride_profile: catalogs.ride_profile_for(ride_key),
            climb,
            waypoints: &ride.waypoints,
            breadcrumb: &recorder.breadcrumb,
            recording: recorder.recording(),
            nav_preview,
            ride_preview,
            detour_preview,
            poi_scratch: &ui.poi_scratch,
            corridor: ui.corridor_scratch.entries(),
            corridor_settled: !ui.corridor_scratch.pending(),
            next_ahead: &ui.next_ahead,
            sensor_status: ui.sensor_status.as_slice(),
            sensor_scan_hits: ui.sensor_scan_hits.as_slice(),
            w: w as i32,
            h: h as i32,
            now_ms: ui.now_ms,
            now_utc,
            now,
            clock_set,
            hold_progress,
            no_fix,
            clock,
            stats: RenderStats::default(),
            fw_version: fw_version.as_str(),
            map_name: map_name.as_str(),
            map_obcm_version: *map_obcm_version,
            card_free_bytes: storage.free_bytes(),
            weather,
            weather_refreshing,
            travel_deg: ride.travel_deg,
            backlight: backlight_available,
        };
        let mut rx = RenderFrame { scene, render: rx };
        // The one Canvas of the frame: every screen draws through it (the base screen — the only
        // possible Map — writes `rx.stats`; the overlays above it leave the stats untouched).
        // A drained region clip makes it reject whole out-of-region primitives — the half of a
        // region-scoped repaint the target's pixel clip can't save (#500 follow-up).
        // A drawer **recesses** the base rather than replacing it: the base draws through the dim
        // LUT composed with the host's own colour policy, the sheet through the untouched one. No
        // capture buffer, no second framebuffer, no alpha for a 64-colour panel to approximate.
        //
        // The switch is a `Cell` inside **one** colour closure rather than a second `Canvas` with a
        // second closure type, and that is not a style choice: `Screen::draw` is generic over the
        // colour function, so a second closure type monomorphises the *entire* screen catalogue and
        // the map renderer a second time — measured at +147 KB of flash on the board. One closure
        // type, and one load-and-branch per **colour resolution** — `Canvas` resolves `color_fn`
        // once per primitive (a span, an outline, a string), so this is O(primitives), not
        // O(pixels).
        let recess = core::cell::Cell::new(ui.stack.iter().skip(base + 1).any(|s| s.is_overlay()));
        let policy = |c: u16| color_fn(if recess.get() { screen::dim_color(c) } else { c });
        // The one Canvas of the frame: every screen draws through it (the base screen — the only
        // possible Map — writes `rx.stats`; the overlays above it leave the stats untouched).
        // A drained region clip makes it reject whole out-of-region primitives — the half of a
        // region-scoped repaint the target's pixel clip can't save (#500 follow-up).
        let mut cv = Canvas::new(target, &policy);
        cv.set_clip(render_clip);
        for (i, scr) in ui.stack.iter().enumerate().skip(base) {
            scr.draw(&mut cv, &mut rx);
            // Everything above the base is the sheet itself, at full colour.
            if i == base {
                recess.set(false);
            }
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
        self.ui.input.render_overlay(target, w, h, &color_fn);
        // The Recalculating banner (issue #1146, P2) rides the same plane as the bulge, and for the
        // same reason: it must appear over a frame the map plane is *not* redrawing. Drawn last so
        // a hold charging during a search still bulges over it.
        if self.reroute_freeze_active() {
            let text = crate::i18n::t(crate::Msg::MapRecalculating, self.settings.language);
            crate::screen::vocab::chrome::recalculating_banner(target, &color_fn, w, h, text);
        }
    }

    /// The Recalculating banner's bounding rows `[y0, y0 + rows)` in a `w`×`h` frame, or `None` when
    /// the freeze is not engaged — the twin of [`InputPlane::overlay_rows`](crate::InputPlane::overlay_rows)
    /// for a partial-overlay host (the board re-presents overlay *rows*, not whole frames). A host
    /// that pushes the union of this and the bulge's rows presents exactly what changed.
    pub fn reroute_banner_rows(&self, h: f32) -> Option<(u16, u16)> {
        self.reroute_freeze_active().then(|| crate::screen::vocab::chrome::recalculating_banner_rows(h))
    }

    /// Whether the overlay plane has live content this frame — a hold bulge charging, popping, or
    /// retracting. `false` exactly when [`render_overlay`](App::render_overlay) would draw nothing,
    /// so a host driving the overlay as a separate layer can leave it idle.
    pub fn overlay_active(&self) -> bool {
        self.ui.input.overlay_active() || self.reroute_freeze_active()
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
    ///
    /// The overlay plane is **derived here, from levels** — the hold bulge's and the Recalculating
    /// freeze's, read as one [`OverlayKey`](crate::device_core::pass::OverlayKey) and folded against
    /// the level this same call last saw. Both rules live in that one converter: see its doc for why
    /// the banner keys on the engaged level rather than on the plan's own start edge.
    pub fn take_dirty(&mut self) -> Dirty {
        let overlay = crate::device_core::pass::OverlayKey {
            hold: self.ui.input.overlay_active(),
            freeze: self.mode.frozen(self.ui.base_draws_map()),
        };
        let overlay = self.pass.overlay_repaint(overlay);
        let mut dirty = self.ui.take_dirty();
        dirty.overlay = overlay;
        dirty
    }

    /// The most recently recognized gesture. No production host reads it; the two-plane input tests
    /// do, to prove the map plane's own recogniser stays dormant when the input plane owns it.
    pub fn last_gesture(&self) -> Option<Gesture> {
        self.ui.input.last_gesture()
    }

    /// Feed the live Select hold-progress (0.0–1.0) for the in-screen confirm fills (the factory
    /// Reset bar). The **two-plane firmware** calls this each frame from its high-priority
    /// [`InputPlane`], whose hold state `App`'s own plane doesn't see — without it the Reset bar
    /// never fills. The single-loop hosts never call it (the render reads `App`'s own input). Pairs
    /// with [`base_draws_map`](App::base_draws_map) + [`top_wants_hold_fill`](App::top_wants_hold_fill):
    /// the host forces a redraw while a hold charges on a cheap screen that would draw the fill, so
    /// it animates (a pure hold-charge doesn't otherwise dirty the map).
    pub fn set_hold_progress(&mut self, progress: f32) {
        self.ui.hold_progress_override = Some(progress);
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
        self.ui.render_clip = clip;
    }

    /// The current operating mode.
    pub fn mode(&self) -> Mode {
        self.activity.mode
    }
}

// ==================== The typed app↔host protocol (FAR-07, #800) ====================
//
// One vocabulary, one pending state. Every host-directed one-shot/counter is drained here as a
// typed [`HostCommand`] through the residual drain — one class, one door, with the pending state
// living once inside `App` (a typed slot, a counter, or a derived predicate).

impl App {
    /// Drain **only** the residual classes — the one a typed executor still performs
    /// ([`device_core::residual`](crate::device_core::residual)).
    ///
    /// **This is not a whole-order walk with a filter afterwards, and the difference is the whole
    /// point.** For every class DeviceCore owns, that walk is not a read: it *pulls* from the domain
    /// — `next_plan_effect`, `SettingsMachine::next_effect`, `DfuState::next_effect`,
    /// `StorageInfo::next_effect`, `next_expiry`, `deliver_plan_cancel` — taking the rider's request
    /// and minting the operation on the way past. A typed executor that walked it would therefore
    /// **destroy** any intent admitted since its own last pass, leave the domain holding an
    /// operation nobody will ever answer, and see the loss only as a command it then declines to
    /// perform.
    ///
    /// That is not hypothetical: it is what a board seam running between the drain and
    /// [`run_pass`](App::run_pass) does on every frame — the debug link's route plan, the phone's
    /// remote update check, a BLE clock stamp arming a settings write. Asking for the class by
    /// name is what makes those seams safe, and it leaves
    /// [`assert_residual`](crate::device_core::residual::assert_residual) as the belt-and-braces
    /// check it was meant to be rather than the thing that notices.
    pub fn drain_residual_commands<const N: usize>(&mut self, out: &mut HostMailbox<N>) -> DrainStatus {
        for class in crate::device_core::residual::RESIDUAL_CLASSES {
            if out.is_full() {
                let remaining = crate::device_core::residual::RESIDUAL_CLASSES
                    .iter()
                    .skip_while(|&&c| c != class)
                    .any(|&c| self.peek_host_command(c));
                return if remaining { DrainStatus::MailboxFull } else { DrainStatus::Complete };
            }
            if let Some(cmd) = self.drain_host_command(class) {
                let pushed = out.push(cmd);
                debug_assert!(pushed, "room was checked before the class was drained");
            }
        }
        DrainStatus::Complete
    }

    /// Whether the **residual** [`ForgetBond`](crate::HostCommand::ForgetBond) is pending — the one
    /// class a typed executor still drains
    /// ([`device_core::residual`](crate::device_core::residual)). Consumes nothing.
    ///
    /// A typed executor's wake is blind to the legacy mailbox, and this class is not an effect, so
    /// `EffectSlots::has_pending` cannot see it either. The removal is posted by one pass and
    /// performed by the **next** pass's drain, so without folding this in that "next pass" is the
    /// next *wake* — and the guarded hold that posts it leaves a static screen, so the next wake is
    /// whenever the rider presses something else.
    ///
    /// The ride save used to be the other half of this. It is a `RecorderEffect` in the pass's own
    /// plan since #1398, so it is covered by the effects and needs no term here.
    ///
    /// Deliberately **not** [`has_pending_host_command`](App::has_pending_host_command): that one
    /// includes the two derived cues, which are levels re-derived on every drain, so folding it
    /// into a wake would spin the loop forever. The residual class is a one-shot the drain clears,
    /// which is what makes this safe to ask for an immediate pass on.
    pub fn has_pending_residual_command(&self) -> bool {
        crate::device_core::residual::RESIDUAL_CLASSES.iter().any(|&c| self.peek_host_command(c))
    }

    /// Whether the "Installing update" card is on the stack — the frame an arming executor freezes
    /// onto the panel for the whole SD→flash stream and the warm reset that never paints.
    ///
    /// [`CardScheduler`](crate::card_scheduler::CardScheduler) can *bounce* the install-began answer
    /// when it has to **push** rather than replace a wait (the debug arm, with no spinner up) and
    /// the stack is full; it re-queues, but a board that armed anyway would have handed the panel a
    /// frame showing something else. This is how it asks.
    pub fn dfu_installing_card_up(&self) -> bool {
        self.ui.stack.iter().any(|s| matches!(s, Screen::DfuInstalling(_)))
    }

    /// Non-consuming per-class pendency for the drain's backpressure check.
    fn peek_host_command(&self, class: HostCommandClass) -> bool {
        match class {
            HostCommandClass::ForgetBond => self.state.ble_forget_pending,
        }
    }

    /// Drain one command class from its single pending slot. Both are one-shots: they drain
    /// exactly once, and a vanished subject consumes the slot and yields nothing.
    fn drain_host_command(&mut self, class: HostCommandClass) -> Option<HostCommand> {
        match class {
            HostCommandClass::ForgetBond => {
                core::mem::take(&mut self.state.ble_forget_pending).then_some(HostCommand::ForgetBond)
            }
        }
    }

    // ==================== keyed derived data (#1437) ====================

    /// What DeviceCore needs read right now — a **level**, recomputed from state, never stored.
    ///
    /// A need stays up until an input carrying *exactly its key* is accepted, and a failure is such
    /// an input, so a dead file costs one read rather than one per pass. Because nothing is stored,
    /// nothing can go stale across a rescan: the key names a durable identity, the source revision
    /// the bytes were last known to change at, and the view generation, so a subject change, a
    /// re-commit or an explicit invalidate all simply produce a different key.
    pub fn derived_needs(&self) -> crate::device_core::derived::DerivedNeeds {
        use crate::device_core::derived::DerivedNeeds;
        let ride_track = self
            .catalogs
            .ride_track_key(self.activity.viewed_ride)
            .filter(|&key| !self.catalogs.ride_track_answered(key));
        // The screen half of the preview level — is an overview up? — is the UI's; the data half is
        // the key's.
        let overview_open = self.ui.stack.iter().any(|s| matches!(s, Screen::RouteOverview(_)));
        let nav_preview = overview_open
            .then(|| self.catalogs.nav_preview_key(self.activity.active_route))
            .flatten()
            .filter(|&key| !self.catalogs.nav_preview_answered(key));
        DerivedNeeds { ride_track, nav_preview }
    }

    /// Accept keyed derived inputs. An input whose key is not the one the need currently carries is
    /// **stale**: it changes nothing at all, and the need stays up.
    ///
    /// One ride-track answer publishes **both** of that need's targets, from the one key: the
    /// profile the executor wrote in place through
    /// [`begin_ride_profile_fill`](App::begin_ride_profile_fill), and the track shape it hands in
    /// through `targets`. They cannot diverge here, which is the point of them sharing a key — the
    /// legacy wrappers reach the same state in two calls only because every host makes both in one
    /// drain.
    ///
    /// Refused while a DeviceCore pass runs: a platform callback must not change DeviceCore
    /// mid-pass, or a later stage would decide from a picture the earlier ones never saw. The pass
    /// reaches the same acceptance through [`accept_derived`](App::accept_derived) at its own stage.
    pub fn apply_derived(
        &mut self,
        inputs: crate::device_core::derived::DerivedInputs,
        targets: crate::device_core::derived::DerivedTargets,
    ) {
        if self.pass.in_pass() {
            // Loud in debug, and refused either way: `run_pass` holds `&mut self` for the whole
            // pass, so reaching here at all means a caller found a way around that borrow.
            debug_assert!(false, "a platform callback cannot change DeviceCore during a pass");
            return;
        }
        self.accept_derived(inputs, targets);
    }

    /// Accept keyed derived inputs — the implementation behind
    /// [`apply_derived`](App::apply_derived) and the pass's own second stage.
    pub(crate) fn accept_derived(
        &mut self,
        inputs: crate::device_core::derived::DerivedInputs,
        targets: crate::device_core::derived::DerivedTargets,
    ) {
        // "The key the need currently carries" is the *need's* key, not the subject's — the two
        // differ, and only this one is right. A nav preview is wanted only while an overview is
        // open, so an answer that lands after the rider closed it is about a question nobody is
        // asking any more; keying on the active route alone would accept it and mark the level
        // answered on a pass that never wanted it.
        let needs = self.derived_needs();
        if let Some(input) = inputs.ride_track {
            let profile = self.catalogs.accept_ride_profile(needs.ride_track, input, None);
            let preview = self.catalogs.accept_ride_preview(needs.ride_track, input, targets.ride_preview);
            if profile || preview {
                self.ui.map_dirty = true;
            }
        }
        if let Some(input) = inputs.nav_preview {
            if self.catalogs.accept_nav_preview(needs.nav_preview, input, targets.nav_preview) {
                self.ui.map_dirty = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_core::derived::{DerivedInput, DerivedInputs, DerivedTargets};
    use crate::settings::SETTINGS_RETRY_BACKOFF_MS;
    use obc_ports::{CompassSource, LocationSource};

    /// A location source that yields one fix then runs dry (so a single `tick` integrates it).
    struct OneFix(Option<Fix>);
    impl LocationSource for OneFix {
        fn poll(&mut self) -> Option<Fix> {
            self.0.take()
        }
    }

    // Per-domain effect helpers keep a test focused on one domain without running a whole frame.
    // Each asks the domain exactly what the pass's own stage asks it.

    /// The ride whose track the open detail still needs — the plan's keyed
    /// [`DerivedNeeds::ride_track`](crate::device_core::DerivedNeeds), read as the durable id.
    fn ride_track_request(app: &App) -> Option<crate::CatalogObjectId> {
        app.derived_needs().ride_track.map(|key| key.ride)
    }

    /// The update domain's next request, if one is owed.
    fn drain_dfu(app: &mut App) -> Option<crate::activity::DfuAction> {
        match app.dfu.next_effect()? {
            crate::dfu::DfuEffect::Scan { .. } => Some(crate::activity::DfuAction::Scan),
            crate::dfu::DfuEffect::ArmInstall { .. } => Some(crate::activity::DfuAction::Install),
        }
    }

    /// Navigator's next route search, if one is owed.
    fn drain_nav(app: &mut App) -> Option<crate::activity::NavRequest> {
        match app.navigator.next_plan_effect(PlanFamily::Route, &mut app.mode)? {
            crate::navigator::NavigatorEffect::Acquire { work: crate::navigator::PlannerWork::Route(req), .. } => {
                Some(req)
            }
            _ => None,
        }
    }

    /// Whether the rider's cancellation of the route search reaches the executor.
    fn drain_cancel(app: &mut App) -> bool {
        app.navigator.next_release(PlanFamily::Route, &mut app.mode).is_some()
    }

    /// The revision a settings write is owed for, if one is. The operation token goes with it, so a
    /// test that only asks (rather than answers) uses this and drops it.
    fn drain_persist(app: &mut App) -> Option<u16> {
        SettingsHost::default().drain(app)
    }

    /// The executor's half of one settings write: it takes the write the app owes and remembers the
    /// operation, so its answer carries the token `SettingsMachine` validates.
    #[derive(Default)]
    struct SettingsHost {
        token: Option<crate::device_core::OperationToken<crate::device_core::SettingsTag>>,
    }

    impl SettingsHost {
        fn drain(&mut self, app: &mut App) -> Option<u16> {
            let (in_subtree, now_ms) = (app.ui.top_is_settings(), app.ui.now_ms);
            let effect =
                app.settings_ops.next_effect(crate::settings::SettingsRecord::Preferences, in_subtree, now_ms)?;
            let crate::settings::SettingsEffect::PersistRevision { token, revision } = effect else {
                panic!("the preferences instance emits its own record, not {effect:?}");
            };
            self.token = Some(token);
            Some(revision)
        }

        /// The write landed.
        fn ack(&mut self, app: &mut App, revision: u16) {
            let token = self.token.take().expect("a write is in flight to answer");
            let now_ms = app.ui.now_ms;
            let _ =
                app.settings_ops.apply_outcome(crate::settings::SettingsOutcome::Persisted { token, revision }, now_ms);
        }

        /// The write failed — the app keeps the revision dirty, re-arms the backoff, and tells the
        /// rider on the shared advisory card.
        fn fail(&mut self, app: &mut App, revision: u16) {
            let token = self.token.take().expect("a write is in flight to answer");
            let now_ms = app.ui.now_ms;
            let outcome = crate::settings::SettingsOutcome::PersistFailed {
                token,
                revision,
                error: obc_ports::SettingsSaveError::Backend,
            };
            if app.settings_ops.apply_outcome(outcome, now_ms) {
                app.on_warning(WarningFlags::SETTINGS_ERROR);
            }
        }
    }

    /// Whether leaving the settings subtree emitted a persist this pass (`take_settings_dirty`).
    fn settings_dirty(app: &mut App) -> bool {
        drain_persist(app).is_some()
    }

    /// Stage 9's offer this pass, whichever record wins the one slot — through the stage's own
    /// seam, since "which record is written, and when" is exactly the question.
    fn drain_settings_effect(app: &mut App) -> Option<crate::settings::SettingsEffect> {
        app.next_settings_effect()
    }

    /// Serve one marks write end to end: take stage 9's offer, "persist" it, and answer it. Returns
    /// the bytes the executor would have written, so a test can round-trip the record.
    fn serve_marks_write(app: &mut App) -> Option<[u8; crate::weather_alerts::ALERT_MARKS_LEN]> {
        let effect = drain_settings_effect(app)?;
        let crate::settings::SettingsEffect::PersistAlertMarks { token, revision } = effect else {
            return None;
        };
        let bytes = crate::weather_alerts::encode_alert_marks(app.alert_marks());
        let outcome = crate::settings::SettingsOutcome::MarksPersisted { token, revision };
        assert!(!app.apply_settings_outcome(outcome), "a durable marks write raises no warning");
        Some(bytes)
    }

    /// The Home root's current backdrop seed.
    fn home_seed(app: &App) -> u32 {
        match app.ui.stack.first() {
            Some(Screen::Home(h)) => h.backdrop_seed(),
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

        app.ui.now_ms = 4242;
        app.apply_gesture(Gesture::BackHold); // Home → Menu (stack grows)
        assert_eq!(home_seed(&app), 0, "going deeper than Home does not reseed");

        app.apply_gesture(Gesture::Back); // Menu → Pop → back to [Home]
        assert_eq!(home_seed(&app), 4242, "returning to Home re-rolls from the wall clock");

        // A gesture Home ignores leaves the stack — and so the pattern — untouched.
        app.ui.now_ms = 9999;
        app.apply_gesture(Gesture::Step(1));
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
    impl obc_ports::AltimeterSource for OneAlt {
        fn poll(&mut self) -> Option<f32> {
            self.0.take()
        }
    }

    /// A clock source that yields one GPS UTC time then runs dry (one fresh stamp per `tick`).
    struct OneClock(Option<obc_ports::GpsTime>);
    impl obc_ports::ClockSource for OneClock {
        fn poll(&mut self) -> Option<obc_ports::GpsTime> {
            self.0.take()
        }
    }

    fn moving(course: f32) -> Fix {
        Fix { lat: 0, lon: 0, course: Some(course), speed_mps: Some(5.0) }
    }

    /// Tick once with only a GPS clock source (no fix / other sensors), at the map-plane clock
    /// `now_ms` — the timebase `wall_clock_now` reads, set here so the stamp + read agree.
    fn tick_clock(app: &mut App, t: obc_ports::GpsTime, now_ms: u32) {
        app.ui.now_ms = now_ms; // mirror `advance_animations(now)` running right before `tick(now)`
        let mut loc = OneFix(None);
        let mut clock = OneClock(Some(t));
        app.tick(RideClock(now_ms), Sensors { clock: Some(&mut clock), ..Sensors::new(&mut loc) }, None);
    }

    fn gps_time(hour: u8, minute: u8, second: u8) -> obc_ports::GpsTime {
        obc_ports::GpsTime { utc: DateTime { year: 2026, month: 6, day: 30, hour, minute }, second }
    }

    /// A fresh boot is **untrusted** — the persisted set-point is display-only until a real source
    /// re-establishes the clock this boot (#641). `clock_is_set` (the coarse "do we know a date?"
    /// gate) can still be true from the seeded set-point; `clock_trusted` (the finer expiry gate) is
    /// not.
    #[test]
    fn boot_clock_is_untrusted() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        assert!(!app.clock_trusted(), "no source stamped the clock yet — untrusted from boot");
        // Even after seeding a persisted set-point, trust stays false: the seed is display-only.
        app.set_settings(Settings {
            clock: DateTime { year: 2026, month: 6, day: 30, hour: 8, minute: 0 },
            ..Settings::default()
        });
        assert!(app.clock_is_set(), "the seeded set-point is established (the Home date line shows)");
        assert!(!app.clock_trusted(), "but a stale persisted seed is never trusted");
    }

    /// GPS **always** stamps now (#641, manual mode gone): a resolved GPS UTC re-stamps the wall
    /// clock to the local time (UTC anchor + offset), marks the clock trusted as `Gps`, and — since
    /// the anchor moved — arms a persist through the change-detected save path.
    #[test]
    fn gps_stamp_sets_clock_and_marks_trusted() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.set_settings(Settings { utc_offset_min: 120, ..Settings::default() });
        assert!(!app.clock_trusted(), "untrusted before the first fix");
        tick_clock(&mut app, gps_time(14, 37, 0), 1000);
        let now = app.wall_clock_now();
        assert_eq!((now.hour, now.minute), (16, 37), "GPS UTC 14:37 + 02:00 → local 16:37");
        assert_eq!(app.settings().clock, gps_time(14, 37, 0).utc, "the stored anchor is the raw UTC");
        assert!(app.clock_trusted(), "a GPS stamp establishes trust this boot");
        assert!(settings_dirty(&mut app), "the moved anchor persists via the settings-save path");
    }

    /// Only the **first trusted stamp of the boot** persists — the boot seed is display-only,
    /// untrusted until re-established next boot, so mid-ride freshness buys nothing. Later same-boot
    /// GPS stamps re-stamp the live clock every fix but never re-arm a save, including a stamp in a
    /// *new* displayed minute (no ride-long RRAM/revision thrash).
    #[test]
    fn only_the_first_trusted_stamp_of_the_boot_persists() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.set_settings(Settings::default());
        tick_clock(&mut app, gps_time(14, 37, 10), 1000);
        assert!(app.clock_trusted(), "the first stamp establishes trust");
        assert!(settings_dirty(&mut app), "the first trusted stamp of the boot persists once");
        // A later fix in the same displayed minute — no re-persist.
        tick_clock(&mut app, gps_time(14, 37, 42), 5000);
        assert!(!settings_dirty(&mut app), "same-minute re-stamp doesn't re-arm a save");
        // A fix in a NEW displayed minute (the anchor moved) — still no re-persist, already trusted.
        tick_clock(&mut app, gps_time(14, 38, 3), 65_000);
        assert!(!settings_dirty(&mut app), "a new-minute re-stamp still doesn't re-persist once trusted");
        assert_eq!(
            (app.wall_clock_now().hour, app.wall_clock_now().minute),
            (14, 38),
            "but the live wall clock still re-stamps every fix",
        );
    }

    /// A BLE `setClock` (epic #638 S2, #642) stamps the wall clock from the phone's unix UTC + live
    /// offset: the displayed time is UTC + offset, the raw UTC anchor is stored, the clock is trusted
    /// as `Ble`, and — the first trusted stamp of the boot — it persists (offset included). The
    /// unix→`DateTime` split + seconds-into-the-minute back-date happen in `stamp_clock_ble`.
    #[test]
    fn ble_setclock_stamps_trusts_and_persists() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.set_settings(Settings::default());
        assert!(!app.clock_trusted(), "untrusted before the first setClock");
        // 2026-07-09T12:00:30Z, +02:00 — the specs/vectors timestamp (unix 1783598400) plus 30 s to
        // exercise the seconds-into-the-minute back-date.
        app.stamp_clock_ble(1_783_598_400 + 30, 120);
        let now = app.wall_clock_now();
        assert_eq!((now.hour, now.minute), (14, 0), "UTC 12:00 + 02:00 → local 14:00");
        assert_eq!(app.settings().clock, DateTime { year: 2026, month: 7, day: 9, hour: 12, minute: 0 });
        assert_eq!(app.settings().utc_offset_min, 120, "the phone's offset is persisted");
        assert_eq!(app.clock_trust, ClockTrust::Ble, "the trust source is BLE");
        assert!(settings_dirty(&mut app), "the first trusted stamp of the boot persists once");
    }

    /// The offset-persistence invariant S2 must hold (#642): a second `setClock` in the **same boot**
    /// carrying a *changed* offset (DST rolled, or the rider flew a timezone) re-persists even though
    /// the clock is already trusted — the change-check sees the move because `stamp_clock` sets the
    /// offset itself before testing it. A reconnect with the *same* offset re-stamps the live clock
    /// but arms no save (no per-connect RRAM thrash).
    #[test]
    fn ble_setclock_persists_a_changed_offset_on_a_same_boot_reconnect() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.set_settings(Settings::default());
        app.stamp_clock_ble(1_783_598_400, 120);
        assert!(settings_dirty(&mut app), "first trusted stamp persists (offset 120)");
        // A later connect the same boot with a *changed* offset (e.g. +01:00 after a flight): already
        // trusted, so `first_trusted_this_boot` is false — only the offset move can arm the save.
        app.stamp_clock_ble(1_783_602_000, 60);
        assert_eq!(app.settings().utc_offset_min, 60, "the new offset is adopted");
        assert!(settings_dirty(&mut app), "a same-boot offset change persists even while already trusted");
        // A reconnect with the same offset: no move, no save.
        app.stamp_clock_ble(1_783_605_600, 60);
        assert!(!settings_dirty(&mut app), "an unchanged offset on reconnect arms no save (no RRAM thrash)");
    }

    /// The seconds-into-the-minute back-date makes the displayed minute roll over at the true
    /// instant, not up to a fix-interval late: a 14:37:56 stamp rolls to 14:38 just 4 s later.
    #[test]
    fn gps_time_back_dates_the_epoch_by_seconds() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.set_settings(Settings::default());
        tick_clock(&mut app, gps_time(14, 37, 56), 10_000); // stamped 56 s into the minute
        assert_eq!((app.wall_clock_now().hour, app.wall_clock_now().minute), (14, 37));
        // 4 s on (56 + 4 = 60 s since the minute's true start) the minute must have rolled.
        app.ui.now_ms = 14_000;
        assert_eq!((app.wall_clock_now().hour, app.wall_clock_now().minute), (14, 38), "rolls 4 s later");
        // Without the back-date the same stamp would still read 14:37 here — 4 s isn't a full minute.
    }

    // --- no-GPS-fix freshness + banner edge ---

    /// Tick once with a single fix at the map-plane clock `now_ms` (set so `last_fix_ms` and
    /// `has_live_fix` share a timebase), no route / other sensors.
    fn tick_fix(app: &mut App, fix: Fix, now_ms: u32) {
        app.ui.now_ms = now_ms; // mirror `advance_animations(now)` running right before `tick(now)`
        let mut loc = OneFix(Some(fix));
        app.tick(RideClock(now_ms), Sensors::new(&mut loc), None);
    }

    /// One **DeviceCore pass** at `now_ms` with a fix and/or a heart-rate reading on the ports,
    /// returning what it planned to repaint. The production frame, and the only composition where
    /// the render keys are compared — a bare `tick` moves the state without ever reaching the
    /// boundary that reads it.
    fn pass_ports(app: &mut App, now_ms: u32, fix: Option<Fix>, bpm: Option<u16>) -> Dirty {
        use crate::device_core::{DerivedInputs, DerivedTargets, ExternalFacts, OutcomeSlots, PassClock, PassInputs};
        let mut outcomes = OutcomeSlots::new();
        let mut facts = ExternalFacts::NONE;
        let mut loc = OneFix(fix);
        let mut hr = OneHr(bpm);
        let plan = app.run_pass(PassInputs {
            now: PassClock { ride: RideClock(now_ms), ui: InputClock(now_ms) },
            gestures: &[],
            sensors: Sensors { hr: Some(&mut hr), ..Sensors::new(&mut loc) },
            route: None,
            weather: None,
            support: crate::harness::support::EVERY_CAPABILITY,
            outcomes: &mut outcomes,
            facts: &mut facts,
            derived: DerivedInputs::NONE,
            targets: DerivedTargets::NONE,
        });
        plan.render
    }

    /// One pass carrying a single fix.
    fn pass_fix(app: &mut App, fix: Fix, now_ms: u32) -> Dirty {
        pass_ports(app, now_ms, Some(fix), None)
    }

    /// One pass with nothing on any port — the quiet frame.
    fn pass_idle(app: &mut App, now_ms: u32) -> Dirty {
        pass_ports(app, now_ms, None, None)
    }

    /// The frozen base, through a **real pass**: a fresh fix under an open drawer moves the camera
    /// and plans no repaint, while the same fix on the bare Map plans one. The render-key tests pin
    /// the mechanism; this pins that the mechanism is what the frame boundary actually reads.
    #[test]
    fn a_fix_under_an_open_drawer_plans_no_repaint() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
        assert!(pass_fix(&mut app, moving(90.0), 1_000).map, "the bare Map repaints on a fresh fix");

        assert!(app.apply_chord(crate::input::Chord::Quick));
        let _ = pass_idle(&mut app, 1_100); // drain the chord's own dirt + the sheet's open frames
        let _ = pass_idle(&mut app, 1_600); // …and the rest of the open animation
        let quiet = pass_fix(&mut app, moving(180.0), 2_000);
        assert!(!quiet.map, "the sheet has settled and the map under it is frozen");
        assert!(app.state.user_fix.is_some(), "…even though the fix landed and moved the camera");
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

    /// The banner's repaint comes from `no_fix` in the map/riding render keys, compared across the
    /// pass: a fix aging into silence repaints the live-data view so the banner appears, and the
    /// first/returning fix repaints it so the banner clears — each exactly once. A stationary
    /// returning fix moves the camera nowhere, so its banner-clear *must* come from the `no_fix`
    /// field, not from the fix that carried it.
    #[test]
    fn no_fix_flip_dirties_the_live_view() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map] → base Map (live data)
        pass_fix(&mut app, Fix::at(0, 0), 1_000); // first fix: banner clears (flip true→false)

        assert!(!pass_idle(&mut app, 3_000).map, "still inside the window → no flip");
        assert!(pass_idle(&mut app, 6_001).map, "fix went stale → banner appears (map dirtied)");
        assert!(!pass_idle(&mut app, 7_000).map, "an unchanged no-fix state doesn't re-dirty");

        // A stationary returning fix recenters the camera onto the spot it already sits, so the only
        // thing that changed is the banner — the clear comes from `no_fix`, not a camera move.
        assert!(pass_fix(&mut app, Fix::at(0, 0), 20_000).map, "fix returned → banner clears");
    }

    /// The flip never dirties a static Home — Home's row declares a key of battery, link and
    /// backdrop, and nothing in it is the banner — so a parked idle device stays clean as a fix ages
    /// out and the "static Home does zero renders" criterion still holds.
    #[test]
    fn no_fix_flip_does_not_dirty_idle_home() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0)); // [Home], Idle — not a live-data view
        pass_fix(&mut app, Fix::at(0, 0), 1_000); // flip true→false, but Home draws no banner
        assert!(!pass_idle(&mut app, 1_000 + 6_001).map, "the no-fix flip never dirties a static Home");
    }

    /// A track sink that counts recorded points.
    #[derive(Default)]
    struct CountSink(usize);
    impl obc_ports::TrackSink for CountSink {
        fn record(&mut self, _p: obc_ports::TrackPoint) -> Result<(), obc_ports::TrackError> {
            self.0 += 1;
            Ok(())
        }
    }

    /// A track sink whose every append fails — the "card pulled / write error mid-ride" case.
    struct FailSink;
    impl obc_ports::TrackSink for FailSink {
        fn record(&mut self, _p: obc_ports::TrackPoint) -> Result<(), obc_ports::TrackError> {
            Err(obc_ports::TrackError)
        }
    }

    /// Starting a ride with no fix yet arms the session immediately (Riding, banner up) but records
    /// nothing and books no moving time — then the first fix logs the segment anchor and clears the
    /// banner ("start before lock").
    #[test]
    fn tracking_arms_without_a_fix_and_records_on_first_fix() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.test_start_ride(); // a route load arms a tracking session

        // A tick with no fix: armed, but nothing recorded and no moving time accrued.
        let mut sink = CountSink::default();
        let mut loc = OneFix(None);
        app.ui.now_ms = 1_000;
        app.tick(RideClock(1_000), Sensors { track: Some(&mut sink), ..Sensors::new(&mut loc) }, None);
        assert!(app.recording(), "the session is armed immediately, fix or not");
        assert!(!app.has_live_fix(1_000), "no fix yet → the banner is up");
        assert_eq!(sink.0, 0, "nothing recorded while searching");
        assert_eq!(app.activity.moving_s, 0.0, "moving time idles until the first fix");

        // The first fix lands → it's logged (the segment anchor) and the banner clears.
        let mut loc = OneFix(Some(Fix::at(0, 0)));
        app.ui.now_ms = 2_000;
        app.tick(RideClock(2_000), Sensors { track: Some(&mut sink), ..Sensors::new(&mut loc) }, None);
        assert!(app.has_live_fix(2_000), "the fix landed → banner clears");
        assert_eq!(sink.0, 1, "the first fix logs the segment anchor");
    }

    /// A failed ride-log append (card pulled / write error mid-ride) must not be swallowed: the app
    /// raises the dismissable "recording error" warning so the rider learns the log dropped a point
    /// — the core of issue #11. Latched once per boot: a whole ride of failing writes is one card.
    #[test]
    fn record_failure_raises_recording_error_warning() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.test_start_ride();

        // No warning while nothing has failed.
        assert!(!app.ui.stack.iter().any(|s| matches!(s, Screen::Warning(_))), "a healthy ride shows no warning card",);

        // A logged fix whose write fails → the recording-error card opens.
        let mut sink = FailSink;
        let mut loc = OneFix(Some(Fix::at(0, 0)));
        app.ui.now_ms = 1_000;
        app.tick(RideClock(1_000), Sensors { track: Some(&mut sink), ..Sensors::new(&mut loc) }, None);
        let card = app
            .ui
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
        assert!(!app.ui.stack.iter().any(|s| matches!(s, Screen::Warning(_))), "dismiss pops the card");
        let mut loc = OneFix(Some(Fix::at(0, 100)));
        app.ui.now_ms = 2_000;
        app.tick(RideClock(2_000), Sensors { track: Some(&mut sink), ..Sensors::new(&mut loc) }, None);
        assert!(
            !app.ui.stack.iter().any(|s| matches!(s, Screen::Warning(_))),
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
        app.tick(RideClock(1000), Sensors { compass: Some(&mut compass), ..Sensors::new(&mut loc) }, None);
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
    /// builds by value, including the KB-scale components. Guards the shared field plan end to end.
    #[test]
    fn init_idle_matches_new_idle() {
        let state = AppState::new(1, 2, 3.0);
        App::new_idle(state).assert_idle_boot_state(state);

        let mut slot = core::mem::MaybeUninit::<App>::uninit();
        // SAFETY: `slot` is a valid, aligned, exclusively-owned region for one `App`.
        let placed = unsafe {
            App::init_idle(slot.as_mut_ptr(), state);
            slot.assume_init_ref()
        };
        placed.assert_idle_boot_state(state);
    }

    /// The **map-first** twins: both paths run the idle plan and then the same map-first tail, so
    /// both land on `[Home, Map]` in Riding with the camera untouched.
    #[test]
    fn init_map_matches_new_map() {
        let state = AppState::new(1, 2, 3.0);
        let by_value = App::new(state);

        let mut slot = core::mem::MaybeUninit::<App>::uninit();
        // SAFETY: `slot` is a valid, aligned, exclusively-owned region for one `App`.
        let placed = unsafe {
            App::init_map(slot.as_mut_ptr(), state);
            slot.assume_init_ref()
        };

        for app in [&by_value, placed] {
            assert_eq!(app.state, state, "the camera state is preserved verbatim");
            assert_eq!(app.activity.mode, Mode::Riding, "map-first boots Riding");
            assert_eq!(app.ui.stack.len(), 2, "exactly Home + Map");
            assert!(matches!(app.ui.stack[0], Screen::Home(_)), "Home stays the always-present root");
            assert!(matches!(app.ui.stack[1], Screen::Map(_)), "the Map is on top");
        }
    }

    // --- end-to-end barometric climb through `tick` ---

    /// Feed one altitude sample through `App::tick`'s `Sensors.altimeter` arm, reading the `climbed`
    /// stat back through the public `App` — the `tick` → `record_altitude` → `climb_m` wiring.
    fn tick_alt(app: &mut App, alt_m: f32, now_ms: u32) {
        let mut loc = OneFix(None); // no fix this tick — isolate the altimeter path
        let mut alt = OneAlt(Some(alt_m));
        app.tick(RideClock(now_ms), Sensors { altimeter: Some(&mut alt), ..Sensors::new(&mut loc) }, None);
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

    // --- end-to-end map-referenced altimeter through `tick` + `sample_terrain` (EL8, #1076) ---

    /// A terrain source at a constant height that counts every sample taken from it — so a test can
    /// assert the *cadence*, not just the value.
    struct FlatTerrain {
        height_m: i16,
        samples: u32,
    }
    impl obc_elevation::ElevationSource for FlatTerrain {
        fn sample(&mut self, _lat_udeg: i32, _lon_udeg: i32) -> Option<i16> {
            self.samples += 1;
            Some(self.height_m)
        }
    }

    /// One host pass: an altitude sample and (optionally) a fresh fix through `tick`, then the
    /// terrain drain the hosts run right behind it.
    fn pass(
        app: &mut App,
        terrain: &mut dyn obc_elevation::ElevationSource,
        fix: Option<Fix>,
        alt_m: f32,
        now_ms: u32,
    ) {
        let mut loc = OneFix(fix);
        let mut alt = OneAlt(Some(alt_m));
        app.ui.now_ms = now_ms;
        app.tick(RideClock(now_ms), Sensors { altimeter: Some(&mut alt), ..Sensors::new(&mut loc) }, None);
        app.sample_terrain(terrain);
    }

    /// A fix at a distinct coordinate each pass, so the matcher/motion path sees real movement.
    fn fix_at(i: u32) -> Fix {
        Fix { lat: 46_650_000 + i as i32 * 100, lon: 8_290_000, course: Some(0.0), speed_mps: Some(5.0) }
    }

    /// The end-to-end unlock: a barometer reading 75 m too high is pulled onto the map's frame, and
    /// the Elevation tile's number follows — while the *recorded* elevation stays raw barometry.
    #[test]
    fn tick_fuses_the_altimeter_onto_the_map_frame() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let mut terrain = FlatTerrain { height_m: 1800, samples: 0 };
        // Before any terrain sample the tile is the plain barometric reading, as it always was.
        pass(&mut app, &mut terrain, Some(fix_at(0)), 1875.0, 1000);
        assert_eq!(app.activity.current_elevation_m(), Some(1875.0), "unsettled → the raw reading");

        for i in 1..40 {
            pass(&mut app, &mut terrain, Some(fix_at(i)), 1875.0, 1000 + i * 1000);
        }
        let shown = app.activity.current_elevation_m().expect("a sample has arrived");
        assert!((shown - 1800.0).abs() < 1.0, "the tile now reads the map-referenced height, got {shown}");
        assert_eq!(app.activity.baro_elevation_m(), Some(1875.0), "the raw barometric reading is untouched");
        assert_eq!(app.activity.track_ele(), 1875, "the RECORDED elevation stays raw barometry");
        assert!(app.activity.altitude().settled());
    }

    /// The cadence contract: terrain is read once per **fresh fix**, no matter how often the host
    /// drains — a per-frame read would be an SD tile fetch on the render path.
    #[test]
    fn terrain_is_sampled_once_per_fix_never_per_frame() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let mut terrain = FlatTerrain { height_m: 900, samples: 0 };
        pass(&mut app, &mut terrain, Some(fix_at(0)), 910.0, 1000);
        assert_eq!(terrain.samples, 1, "one fix, one sample");
        // Ten more host passes with no fresh fix at all — the drain must find nothing pending.
        for i in 0..10 {
            assert!(!app.sample_terrain(&mut terrain), "no fresh fix → nothing to sample");
            pass(&mut app, &mut terrain, None, 911.0 + i as f32, 2000 + i * 100);
        }
        assert_eq!(terrain.samples, 1, "still exactly one terrain read");
        pass(&mut app, &mut terrain, Some(fix_at(1)), 910.0, 9000);
        assert_eq!(terrain.samples, 2, "the next fresh fix takes exactly one more");
    }

    /// A map with no terrain beside it: the null source answers nothing, so the estimator never
    /// settles and the tile is bit-for-bit its pre-epic self. The "removing terrain changes nothing
    /// else" contract, at the app's top seam.
    #[test]
    fn a_terrain_less_map_leaves_the_elevation_tile_exactly_as_it_was() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let mut null = obc_elevation::NullElevation;
        for i in 0..60 {
            pass(&mut app, &mut null, Some(fix_at(i)), 640.0 + i as f32, 1000 + i * 1000);
        }
        assert!(!app.activity.altitude().settled(), "no residual ever arrived");
        assert_eq!(app.activity.altitude().offset_m(), None);
        assert_eq!(app.activity.current_elevation_m(), app.activity.baro_elevation_m());
        assert_eq!(app.activity.current_elevation_m(), Some(699.0));
    }

    // --- end-to-end BLE sensor seam through `tick` (SE2, #709) ---

    /// A heart-rate strap that yields one sample then runs dry (the fresh-mailbox contract).
    struct OneHr(Option<u16>);
    impl obc_ports::HeartRateSource for OneHr {
        fn poll(&mut self) -> Option<u16> {
            self.0.take()
        }
    }

    /// A power meter that yields one sample then runs dry.
    struct OnePower(Option<u16>);
    impl obc_ports::PowerSource for OnePower {
        fn poll(&mut self) -> Option<u16> {
            self.0.take()
        }
    }

    /// A cadence sensor that yields one sample then runs dry.
    struct OneCadence(Option<u8>);
    impl obc_ports::CadenceSource for OneCadence {
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
        app.ui.now_ms = 2_000;
        let mut loc = OneFix(Some(Fix::at(STEP_UD, 0)));
        let mut hr = OneHr(Some(150));
        let mut power = OnePower(Some(250));
        let mut cadence = OneCadence(Some(90));
        app.tick(
            RideClock(2_000),
            Sensors {
                hr: Some(&mut hr),
                power: Some(&mut power),
                cadence: Some(&mut cadence),
                ..Sensors::new(&mut loc)
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
    /// record on — **not** the render-time `self.ui.now_ms`. On the board those are one monotonic `now`;
    /// in the simulator mid GPX replay they diverge (record on playback time, render on wall time),
    /// and a tile keyed on the render clock blanked to `--` within `SENSOR_STALE_MS` — Timo's "the
    /// values showed up once, then only dashes." This pins the fix: `_display` stays fresh across the
    /// divergence, while the raw render-clock read is what used to (wrongly) blank.
    #[test]
    fn sensor_tile_display_survives_render_clock_divergence() {
        // The old sim mid-replay: sample recorded on playback time (30 s), but the render/map-plane
        // clock ran on wall time (90 s) — a 60 s gap > SENSOR_STALE_MS.
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.ui.now_ms = 90_000; // wall clock, far ahead of the replay's playback clock
        let mut loc = OneFix(None);
        let mut hr = OneHr(Some(142));
        app.tick(
            RideClock(30_000), // playback time — the clock the HR sample records on
            Sensors { hr: Some(&mut hr), ..Sensors::new(&mut loc) },
            None,
        );
        // The tile path: fresh, because it compares against the recorded-on clock (30 s), not 90 s.
        assert_eq!(app.activity.live_hr_display(), Some(142), "the tile shows the value across the divergence");
        // The old, wrong path — reading against the render clock — is what blanked the tile.
        assert_eq!(
            app.activity.live_hr(app.ui.now_ms),
            None,
            "the render-clock read is stale (90 s vs a 30 s sample) — the bug `_display` fixes"
        );

        // And staleness still works on the ride clock: advance the tick clock 6 s past the sample
        // with no new reading → the tile blanks, exactly as a dropped strap should.
        app.activity.note_sensor_clock(36_001);
        assert_eq!(app.activity.live_hr_display(), None, "a >5 s-old sample still blanks — no frozen value");
    }

    /// One **pass** with only an HR sample (no fix, nothing else moving): `loc` yields `None`, so
    /// the camera and the fix compare equal across it and any repaint is the grid's own.
    fn pass_hr_only(app: &mut App, bpm: Option<u16>, at_ms: u32) -> Dirty {
        pass_ports(app, at_ms, None, bpm)
    }

    /// An app parked on the Statistics grid — the one screen that draws the live sensor tiles — with
    /// the idle return off so a multi-second replay is not swept back to Home mid-assertion.
    fn on_statistics(fields: crate::stat_fields::StatFieldList) -> App {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        // One page of fields, so the grid's own auto-cycle timer never fires and every repaint in
        // these tests is the one under test. The idle return is off for the same reason.
        app.set_settings(Settings {
            idle_return: crate::settings::IdleReturn::Never,
            stat_fields: fields,
            ..Settings::default()
        });
        app.ui.stack.clear();
        let _ = app.ui.stack.push(Screen::Home(crate::screen::HomeScreen::new()));
        let _ = app.ui.stack.push(Screen::Statistics(crate::screen::StatisticsScreen::new()));
        pass_idle(&mut app, 0); // the host's mandatory first frame — drained so each assertion is its own
        app
    }

    /// Epic #744 SR3: a fresh BLE sample lands in `Activity`, which the old `AppState` comparison
    /// never saw — so with an HR tile pinned, the tile froze until something *else* (a moving fix,
    /// reopening the screen) happened to repaint. Now the grid's row declares those values in its
    /// render key: a changed displayed value repaints the grid exactly once, an unchanged one
    /// doesn't, and the 5 s staleness expiry (the blank to `--`) moves the key too.
    #[test]
    fn fresh_sensor_sample_repaints_the_riding_view() {
        let mut app = on_statistics(crate::stat_fields::StatFieldList::decode(
            1,
            &[crate::stat_fields::StatField::HeartRate as u8],
        ));

        assert!(pass_hr_only(&mut app, Some(155), 1_000).map, "a fresh HR sample must repaint the grid");
        // A new sample with the same displayed value is not a change.
        assert!(!pass_hr_only(&mut app, Some(155), 2_000).map, "an unchanged displayed value must not re-dirty");
        assert!(pass_hr_only(&mut app, Some(156), 3_000).map, "a changed bpm repaints again");

        // The strap drops: >5 s later the staleness gate blanks the tile — that flip must paint
        // (once), or the rider stares at a frozen last value.
        assert!(pass_hr_only(&mut app, None, 9_001).map, "the staleness expiry (value → `--`) must repaint");
        assert!(!pass_hr_only(&mut app, None, 20_000).map, "still blank → no re-dirty");
    }

    /// The economy half of the SR3 edge: with **no sensor tile pinned** (the default six fields), a
    /// notification stream must never force map renders — the key omits the quantity entirely, which
    /// is the same economy the per-quantity guards used to spell out by hand.
    #[test]
    fn sensor_sample_without_a_pinned_tile_never_repaints() {
        let mut app =
            on_statistics(crate::stat_fields::StatFieldList::decode(1, &[crate::stat_fields::StatField::Speed as u8]));
        assert!(!pass_hr_only(&mut app, Some(155), 1_000).map, "no HR tile pinned → no forced render");
    }

    /// And off the grid entirely (Home is the base), a pinned tile still doesn't repaint — Home's
    /// key names battery, link and backdrop, and no sensor value; entering Statistics repaints on
    /// the screen change anyway.
    #[test]
    fn sensor_sample_on_home_never_repaints() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0)); // base = Home
        assert!(app.settings.stat_fields.push(crate::stat_fields::StatField::HeartRate));
        pass_idle(&mut app, 0); // drain the boot frame
        assert!(!pass_hr_only(&mut app, Some(155), 1_000).map, "Home draws no tiles → no repaint");
    }

    /// The Map draws the chips, the route line and the marker — never a sensor tile. A pinned HR
    /// field must therefore not wake a ~97 ms map render at the strap's notification rate, which is
    /// the one repaint the old base-screen gate could not tell apart from the grid's.
    #[test]
    fn sensor_sample_on_the_map_never_repaints() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map] — a map base
        assert!(app.settings.stat_fields.push(crate::stat_fields::StatField::HeartRate));
        pass_idle(&mut app, 0); // drain the boot frame
        assert!(!pass_hr_only(&mut app, Some(155), 1_000).map, "the Map draws no tiles → no repaint");
    }

    // --- settings persistence signal (the host's save trigger) ---

    /// A settings edit flags a save, but **debounced to leaving the settings subtree**: while still
    /// on a settings screen the pending edit is held (coalescing a multi-step edit into one
    /// write), surfacing once on the frame after navigating out.
    #[test]
    fn a_settings_edit_flags_dirty_on_leaving_the_settings_subtree() {
        use crate::settings::Units;
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        // Walk to the Units screen: Settings list → System (the last group) → Units (its first row).
        app.apply_gesture(Gesture::BackHold); // Home → Menu
        app.apply_gesture(Gesture::Step(-1)); // → Settings entry (wraps back from Routes)
        app.apply_gesture(Gesture::Press); // → Settings list
        app.apply_gesture(Gesture::Step(-1)); // → System row (last, wraps up from Ride)
        app.apply_gesture(Gesture::Press); // → System menu (Units is the first row)
        app.apply_gesture(Gesture::Press); // → Units screen
        assert!(!settings_dirty(&mut app), "navigation changed no setting, so nothing to save");

        let before = app.settings().units;
        app.apply_gesture(Gesture::Press); // flip units (live immediately, but persistence is debounced)
        assert_ne!(app.settings().units, before, "the Units screen flipped the system");
        assert_eq!(app.settings().units, Units::Imperial, "default Metric → Imperial");
        assert!(!settings_dirty(&mut app), "still on a settings screen → the save is held, not fired per step");

        app.apply_gesture(Gesture::Back); // Units → System menu (still inside the settings subtree)
        assert!(!settings_dirty(&mut app), "the System menu is itself a settings screen — save stays held");

        app.apply_gesture(Gesture::Back); // System menu → Settings list (still inside the settings subtree)
        assert!(!settings_dirty(&mut app), "the Settings list is itself a settings screen — save stays held");

        app.apply_gesture(Gesture::Back); // Settings list → Menu (left the settings subtree)
        assert!(settings_dirty(&mut app), "leaving settings flushes the pending edit — one coalesced save");
        assert!(!settings_dirty(&mut app), "and the flag drains — only saved once");
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
            apply, AddFieldScreen, ConnectionsScreen, DateTimeScreen, FirmwareScreen, PowerScreen, ResetScreen,
            RideScreen, SettingsScreen, StatFieldsScreen, SystemScreen, Transition, UnitsScreen, WeatherSettingsScreen,
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
        let cases: [Case; 12] = [
            // Pure navigation — no edit gesture of its own.
            ("Settings list", || one(Screen::Settings(SettingsScreen::new())), &[]),
            // Open the UTC-offset stepper (#641: the one editable row), +one step — and leave the
            // field open, so Back must still close it then exit.
            ("Date & Time", || one(Screen::DateTime(DateTimeScreen::new())), &[Gesture::Press, Gesture::Step(1)]),
            // Press flips metric ↔ imperial.
            ("Units", || one(Screen::Units(UnitsScreen::new())), &[Gesture::Press]),
            // → the Page-cycle row (index 2), open its stepper, +1 s (and leave it open — Back must
            // still close it then exit).
            ("Ride", || one(Screen::Ride(RideScreen::new())), &[Gesture::Step(2), Gesture::Press, Gesture::Step(1)]),
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
            // Pure navigation — the Connections menu only opens its pages.
            ("Connections", || one(Screen::Connections(ConnectionsScreen::new())), &[]),
            // → the Power Saver row, flip it.
            ("Power", || one(Screen::Power(PowerScreen::new())), &[Gesture::Step(1), Gesture::Press]),
            // Pure navigation — the System menu only opens its pages.
            ("System", || one(Screen::System(SystemScreen::new())), &[]),
            // Pure navigation — the Firmware page's install action leaves the settings subtree.
            ("Firmware", || one(Screen::Firmware(FirmwareScreen::new())), &[]),
            // Press arms, then the completed hold erases to defaults — a real diff off the seed below.
            ("Reset", || one(Screen::Reset(ResetScreen::new())), &[Gesture::Press, Gesture::Hold]),
            // Open the refresh picker, step it once (and leave it open — Back closes it first).
            (
                "Weather",
                || one(Screen::WeatherSettings(WeatherSettingsScreen::new())),
                &[Gesture::Press, Gesture::Step(1)],
            ),
        ];

        for (name, stack, edits) in cases {
            let mut app = App::new_idle(AppState::new(0, 0, 1.0));
            // A non-default seed, so the factory Reset's erase-to-defaults really changes something.
            app.set_settings(Settings { units: Units::Imperial, ..Settings::default() });
            for s in stack() {
                apply(&mut app.ui.stack, Transition::Push(s));
            }
            assert!(app.ui.top_is_settings(), "{name} must classify as ScreenKind::Settings");

            let before = *app.settings();
            for &g in edits {
                app.apply_gesture(g);
            }
            if edits.is_empty() {
                app.arm_settings_save();
            } else {
                assert_ne!(*app.settings(), before, "{name}: the edit script changed a setting");
            }
            assert!(!settings_dirty(&mut app), "{name}: the save is held while the screen is on top");

            // Back out to the Home root (closing any open field on the way); the save stays held
            // for as long as any settings screen remains on top, then flushes exactly once.
            for _ in 0..MAX_DEPTH_BACKOUT {
                if app.ui.stack.len() == 1 {
                    break;
                }
                assert!(!settings_dirty(&mut app), "{name}: still inside the settings subtree — save held");
                app.apply_gesture(Gesture::Back);
            }
            assert_eq!(app.ui.stack.len(), 1, "{name}: backed out to the Home root");
            assert!(settings_dirty(&mut app), "{name}: leaving the settings subtree flushes the pending save");
            assert!(!settings_dirty(&mut app), "{name}: the flag drains — exactly one save");
        }
    }

    /// Upper bound of `Back` presses needed to unwind any settings case above (open field + the
    /// stacked screens), safely under test control rather than looping forever on a regression.
    const MAX_DEPTH_BACKOUT: usize = crate::screen::MAX_DEPTH;

    // --- device warning card (issue #504) ---

    /// The deepest ordinary mid-ride settings path leaves the same two-slot reserve the stack had
    /// before the ride-scoped and main-menu ancestors were added. Walk the real navigation with
    /// gestures, then prove a host-pushed warning can still land instead of being silently dropped.
    #[test]
    fn deepest_mid_ride_settings_path_keeps_room_for_host_warning() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.set_routes_with_ids(&[summary("Alpha")], &[10]);

        app.apply_gesture(Gesture::Press); // Home → Menu (Routes selected)
        app.apply_gesture(Gesture::Press); // Menu → Route menu
        app.apply_gesture(Gesture::Press); // Route menu → Overview
        app.apply_gesture(Gesture::Press); // START → [Home, Map]
        assert!(matches!(app.top_screen(), Screen::Map(_)));

        app.apply_gesture(Gesture::BackHold); // Map → Ride menu
        app.apply_gesture(Gesture::Step(-1)); // Waypoints → Main menu
        app.apply_gesture(Gesture::Press); // Ride menu → Menu (Push, preserving ride caller)
        app.apply_gesture(Gesture::Step(-1)); // Routes → Settings
        app.apply_gesture(Gesture::Press); // Menu → Settings
        app.apply_gesture(Gesture::Press); // Settings → Ride
        app.apply_gesture(Gesture::Step(1)); // Bike type → Data fields
        app.apply_gesture(Gesture::Press); // Ride → Fields
        let field_count = app.settings().stat_fields.len();
        app.apply_gesture(Gesture::Step(field_count as i32)); // first field → trailing Add tile
        app.apply_gesture(Gesture::Press); // Fields → Add field

        assert!(matches!(app.top_screen(), Screen::AddField(_)), "the deepest normal path is open");
        assert_eq!(app.ui.stack.len(), 8, "the full mid-ride settings path occupies eight slots");
        assert_eq!(crate::screen::MAX_DEPTH - app.ui.stack.len(), 2, "two host-card slots stay reserved");

        app.on_warning(WarningFlags::REC_ERROR);
        assert_eq!(app.ui.stack.len(), 9, "the host warning pushes over the deepest normal path");
        match app.top_screen() {
            Screen::Warning(w) => assert!(w.flags().contains(WarningFlags::REC_ERROR)),
            _ => panic!("the recording-error warning must not be dropped at maximum normal depth"),
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
        assert!(!settings_dirty(&mut app), "seeding the boot value must not trigger a write-back");
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
        app.ui.now_ms = 25 * 60_000; // 25 minutes of monotonic time later
        let now = app.wall_clock_now();
        assert_eq!((now.hour, now.minute), (15, 5), "the clock advanced 25 min, carrying into the hour");
    }

    /// Turning the UTC offset on the Date & Time screen re-stamps the wall clock to the new local
    /// time (the one surviving clock edit — manual date/time was removed in #641). Drives the real
    /// navigation (Home → Menu → Settings → System → Date & Time → offset field).
    #[test]
    fn offset_edit_restamps_the_wall_clock_to_local() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.set_settings(Settings {
            clock: DateTime { year: 2026, month: 6, day: 29, hour: 12, minute: 0 }, // UTC anchor
            utc_offset_min: 0,
            ..Settings::default()
        });
        app.apply_gesture(Gesture::BackHold); // Home → Menu
        app.apply_gesture(Gesture::Step(-1)); // → Settings entry (wraps back from Routes)
        app.apply_gesture(Gesture::Press); // → Settings list
        app.apply_gesture(Gesture::Step(-1)); // → System row (last, wraps up)
        app.apply_gesture(Gesture::Press); // → System menu (Units is row 0)
        app.apply_gesture(Gesture::Step(1)); // → Date & Time row (1)
        app.apply_gesture(Gesture::Press); // → Date & Time (cursor parked on the offset row)
        app.apply_gesture(Gesture::Press); // open the offset field
        app.apply_gesture(Gesture::Step(1)); // +one step (+15 min)
        assert_eq!(app.settings().utc_offset_min, crate::settings::UTC_OFFSET_STEP, "the offset stepped one step");
        let now = app.wall_clock_now();
        assert_eq!((now.hour, now.minute), (12, 15), "the offset re-stamped the wall clock to local = UTC + offset");
    }

    /// The Home wall clock shows **local** time (the UTC anchor shifted by the offset), so it agrees
    /// with the Date & Time screen's "Local time" row instead of trailing it.
    #[test]
    fn wall_clock_shows_local_time() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let seeded = crate::settings::Settings {
            clock: DateTime { year: 2026, month: 6, day: 29, hour: 12, minute: 0 }, // the UTC anchor
            utc_offset_min: 120,                                                    // +02:00
            ..Default::default()
        };
        app.set_settings(seeded);
        let now = app.wall_clock_now();
        assert_eq!((now.hour, now.minute), (14, 0), "Home shows local = UTC + offset, not the raw UTC anchor");
        assert_eq!(now, seeded.local_clock(), "and it matches the local_clock the Local time row reads");
    }

    /// The `Next: <category>` tiles' whole-App seam (epic #946, U5): the per-category cache asks for
    /// a corridor snapshot **only** where the answer is read — the Statistics screen, with such a
    /// tile placed and a route loaded — and the request is a *single*-category one anchored at live
    /// progress. Everywhere else the scratch stays disarmed and the board never builds a `Reader`
    /// for it.
    #[test]
    fn next_category_tiles_ask_for_a_corridor_only_where_they_are_drawn() {
        use crate::stat_fields::{StatField, StatFieldList};
        use obc_reader::{PoiCategory, PoiCategorySet};

        let mut app = App::new(AppState::new(0, 0, 1.0)); // base = the riding Map
        app.test_start_ride();
        app.activity.active_route = Some(0);
        app.activity.progress_m = 1_500;

        // No `Next:` tile on the grid: nothing is ever asked for, wherever the rider is.
        app.advance_animations(InputClock(1_000));
        assert!(!app.corridor_snapshot_pending(), "the default grid asks for nothing");
        app.apply_gesture(Gesture::Back); // Map → Statistics
        assert!(matches!(app.top_screen(), Screen::Statistics(_)));
        app.advance_animations(InputClock(2_000));
        assert!(!app.corridor_snapshot_pending(), "…not even on the stats page");

        // Place one. The cache now wants exactly that category, anchored at live progress.
        let mut fields = StatFieldList::decode(0, &[]);
        assert!(fields.push(StatField::NextWater));
        app.set_settings(crate::settings::Settings { stat_fields: fields, ..Default::default() });
        app.advance_animations(InputClock(3_000));
        assert_eq!(
            app.ui.corridor_scratch.armed(),
            Some(crate::corridor::CorridorKey { filter: PoiCategorySet::only(PoiCategory::Water), anchor_m: 1_500 }),
            "one category per query (the 16-result cap makes a union query unable to answer)"
        );
        assert!(app.corridor_snapshot_pending(), "…and the host is asked for the Reader until it lands");

        // Leave the stats page: the request goes away with it, Reader seam quiet again.
        app.apply_gesture(Gesture::Back);
        assert!(!matches!(app.top_screen(), Screen::Statistics(_)));
        app.advance_animations(InputClock(4_000));
        assert!(!app.corridor_snapshot_pending(), "a tile nobody is looking at costs nothing");

        // A route-less ride never asks either — there is no "ahead" to answer with.
        app.activity.active_route = None;
        app.apply_gesture(Gesture::Back);
        app.advance_animations(InputClock(5_000));
        assert!(!app.corridor_snapshot_pending());
    }

    /// A **screen** always outranks the stat-field cache for the one shared corridor scratch: opening
    /// the Up-ahead list re-points it at the list's own key, and the cache's request waits (its
    /// harvest only ever accepts its own key, so the list's snapshot can't land in a tile).
    #[test]
    fn an_up_ahead_screen_outranks_the_stat_field_cache() {
        use crate::stat_fields::{StatField, StatFieldList};
        use obc_reader::{PoiCategory, PoiCategorySet};

        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.test_start_ride();
        app.activity.active_route = Some(0);
        app.activity.progress_m = 2_000;
        let mut fields = StatFieldList::decode(0, &[]);
        assert!(fields.push(StatField::NextPharmacy));
        app.set_settings(crate::settings::Settings { stat_fields: fields, ..Default::default() });

        app.apply_gesture(Gesture::Back); // → Statistics
        app.advance_animations(InputClock(1_000));
        assert_eq!(
            app.ui.corridor_scratch.armed().map(|k| k.filter),
            Some(PoiCategorySet::only(PoiCategory::Pharmacy)),
            "the cache holds the scratch while nothing else wants it"
        );

        app.apply_gesture(Gesture::BackHold); // → the ride menu
        app.apply_gesture(Gesture::Press); // → Up ahead (the north station)
        assert!(matches!(app.top_screen(), Screen::UpAhead(_)));
        app.advance_animations(InputClock(2_000));
        assert_eq!(
            app.ui.corridor_scratch.armed().map(|k| k.filter),
            Some(PoiCategorySet::ALL),
            "the screen's own key wins the shared buffer"
        );
    }

    /// The **whole loop, through real frames** (epic #946, U5): the cache arms a single-category
    /// corridor request on the stats page, the pre-draw `prepare` boundary runs the query off the map
    /// `Reader` and distils entry `0` into the cache, the tile then reads it — and riding on inside
    /// [`REFRESH_STEP_M`](crate::next_ahead::REFRESH_STEP_M) re-queries **nothing** while crossing it
    /// re-arms exactly once. The query count is the point: the same seam per-frame would be an SD
    /// read per frame.
    #[test]
    fn the_next_category_cache_fills_from_a_real_frame_and_then_goes_quiet() {
        use crate::stat_fields::{StatField, StatFieldList};
        use embedded_graphics::pixelcolor::Rgb888;
        use obc_formats::io::{ByteSink, SliceSource};
        use obc_reader::{MapCache, MapTables, PoiCategory, Reader};
        use obc_route::{RouteIndex, RouteReader};
        use obcm_testkit::{build_poi_map, PoiSpec};

        /// A `ByteSink` over a growable `Vec` — "write the .obcr to RAM".
        #[derive(Default)]
        struct VecSink(std::vec::Vec<u8>);
        impl ByteSink for VecSink {
            fn write(&mut self, b: &[u8]) -> Result<(), obc_formats::io::Error> {
                self.0.extend_from_slice(b);
                Ok(())
            }
            fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), obc_formats::io::Error> {
                let o = off as usize;
                self.0[o..o + b.len()].copy_from_slice(b);
                Ok(())
            }
        }
        /// A `DrawTarget` that keeps nothing — these frames are run for their `prepare` pass.
        struct Sink;
        impl embedded_graphics::prelude::Dimensions for Sink {
            fn bounding_box(&self) -> Rectangle {
                Rectangle::new(
                    embedded_graphics::prelude::Point::zero(),
                    embedded_graphics::prelude::Size::new(240, 320),
                )
            }
        }
        impl embedded_graphics::prelude::DrawTarget for Sink {
            type Color = Rgb888;
            type Error = core::convert::Infallible;
            fn draw_iter<I>(&mut self, _: I) -> Result<(), Self::Error>
            where
                I: IntoIterator<Item = embedded_graphics::Pixel<Self::Color>>,
            {
                Ok(())
            }
        }

        // A due-east route with three water points beside it, at ~1.7 km / 3.5 km / 5.2 km along.
        let mut gpx = std::string::String::from(r#"<?xml version="1.0"?><gpx version="1.1"><trk><trkseg>"#);
        for i in 0..30 {
            let lon = 7.8000 + 0.0020 * i as f64;
            gpx.push_str(&std::format!(r#"<trkpt lat="48.0000" lon="{lon:.4}"><ele>200.0</ele></trkpt>"#));
        }
        gpx.push_str("</trkseg></trk></gpx>");
        let mut sink = VecSink::default();
        obc_route::gpx_to_obcr(&SliceSource(gpx.as_bytes()), "East", &mut sink).unwrap();
        let obcr = sink.0;
        let map = build_poi_map(
            (7_000_000, 47_000_000, 9_000_000, 49_000_000),
            512,
            &[(
                1,
                std::vec![
                    PoiSpec { lat: 48_001_000, lon: 7_823_000, subtype: 1, name: "Brunnen".into(), hours_ref: 0xFFFF },
                    PoiSpec { lat: 47_999_000, lon: 7_847_000, subtype: 1, name: "Spring".into(), hours_ref: 0xFFFF },
                ],
            )],
        );

        // One rendered frame with both inputs — the shape the board produces when
        // `base_needs_reader` says it must.
        let frame = |app: &mut App| {
            let cache = MapCache::new();
            let map_src = SliceSource(&map);
            let tables = MapTables::parse(&map_src).expect("valid .obcm");
            let reader = Reader::new(&map_src, &tables, &cache);
            let route_src = SliceSource(&obcr);
            let idx = RouteIndex::read(&route_src).expect("valid .obcr");
            let route = RouteReader::new(&idx, &route_src);
            let mut scratch = Box::new(RenderScratch::new());
            app.render_frame(Some(&mut scratch), &mut Sink, &reader, Some(&route), 240.0, 320.0, |_| {
                Rgb888::new(0, 0, 0)
            });
        };

        let mut app = App::new(AppState::new(7_800_000, 48_000_000, 0.05));
        app.test_start_ride();
        app.activity.active_route = Some(0);
        let mut fields = StatFieldList::decode(0, &[]);
        assert!(fields.push(StatField::NextWater));
        app.set_settings(crate::settings::Settings { stat_fields: fields, ..Default::default() });
        app.apply_gesture(Gesture::Back); // Map → Statistics
        assert!(matches!(app.top_screen(), Screen::Statistics(_)));

        app.advance_animations(InputClock(1_000));
        assert!(app.base_needs_reader(), "the armed refresh keeps the Reader built");
        frame(&mut app);
        assert!(!app.base_needs_reader(), "…exactly until the snapshot lands, then it stops");
        let first = app.ui.next_ahead.poi(PoiCategory::Water).expect("the nearest water ahead is cached");
        assert_eq!(first.name.as_str(), "Brunnen", "entry 0 of a single-category query is the nearest");
        let brunnen_m = first.dist_along_m;
        assert!((1_500..2_000).contains(&brunnen_m), "…projected onto the route axis, got {brunnen_m}");

        // Ride on inside the step: many frames, not one further query.
        for m in (100..500).step_by(50) {
            app.activity.progress_m = m;
            app.advance_animations(InputClock(2_000 + m));
            assert!(!app.base_needs_reader(), "no re-query inside the refresh step (at {m} m)");
            frame(&mut app);
        }
        assert_eq!(app.ui.next_ahead.poi(PoiCategory::Water).map(|p| p.dist_along_m), Some(brunnen_m));

        // Cross the step: exactly one re-take, and the answer is unchanged (nothing was passed).
        app.activity.progress_m = crate::next_ahead::REFRESH_STEP_M;
        app.advance_animations(InputClock(9_000));
        assert!(app.base_needs_reader(), "crossing the step re-arms");
        frame(&mut app);
        assert!(!app.base_needs_reader(), "and settles again on the very next eligible frame");
        assert_eq!(app.ui.next_ahead.poi(PoiCategory::Water).map(|p| p.dist_along_m), Some(brunnen_m));

        // Ride past the cached fountain: the re-take hands the tile the next one along.
        app.activity.progress_m = brunnen_m + 10;
        app.advance_animations(InputClock(10_000));
        frame(&mut app);
        assert_eq!(
            app.ui.next_ahead.poi(PoiCategory::Water).map(|p| p.name.as_str().into()),
            Some(std::string::String::from("Spring")),
            "a passed entry re-arms out of turn and the next one takes its place"
        );
    }

    /// A **same-index / new-bytes** route replace invalidates the `Next: <category>` cache (epic
    /// #946, U5). The cache keys its identity on the catalog index, and a replace leaves that index
    /// (and the id) exactly where it was — so nothing inside `NextAhead` can see the swap, and its
    /// along-route distances would go on naming places on geometry that no longer exists. The
    /// `App`-level `drop_route_derived_state` seam is what tells it, alongside the matcher and the
    /// profile/climb/waypoint caches dropped for the identical reason.
    ///
    /// Deliberately pinned with progress at **0**: that is the case the progress-rewind trigger
    /// cannot cover (there is nothing to rewind from), so it isolates the invalidation itself.
    #[test]
    fn a_same_index_route_replace_invalidates_the_next_category_cache() {
        use crate::stat_fields::{StatField, StatFieldList};
        use obc_reader::{CorridorPoi, Poi, PoiCategory, PoiCategorySet};

        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.test_start_ride();
        app.set_routes_with_ids(&[summary("East")], &[10]);
        app.activate_route(0);
        let mut fields = StatFieldList::decode(0, &[]);
        assert!(fields.push(StatField::NextWater));
        app.set_settings(crate::settings::Settings { stat_fields: fields, ..Default::default() });
        app.apply_gesture(Gesture::Back); // Map → Statistics
        assert!(matches!(app.top_screen(), Screen::Statistics(_)));

        // Fill the slot the way a landed snapshot would, at the very start of the route.
        app.advance_animations(InputClock(1_000));
        let key = app.ui.next_ahead.request().expect("the placed tile arms a refresh");
        assert_eq!(key.filter, PoiCategorySet::only(PoiCategory::Water));
        let mut name = heapless::String::new();
        name.push_str("Old bytes").unwrap();
        app.ui.next_ahead.harvest(
            key,
            &[CorridorPoi {
                poi: Poi { lat: 0, lon: 0, subtype: 1, name, hours_ref: 0xFFFF, distance_m: 5_000 },
                dist_along_m: 5_000,
                offset_m: 0,
            }],
        );
        app.advance_animations(InputClock(2_000));
        assert_eq!(app.ui.next_ahead.request(), None, "settled on the old geometry's answer");

        // The phone re-uploads route 10 over itself: same id, same index, new bytes.
        app.on_route_uploaded(10, true, None);
        assert_eq!(app.ui.next_ahead.poi(PoiCategory::Water), None, "the old geometry's answer is dropped");

        // Dismiss the advisory "route updated" card back to the grid (the tiles only refresh while
        // Statistics is the base screen).
        assert!(matches!(app.top_screen(), Screen::RouteUpdated(_)));
        app.apply_gesture(Gesture::Press);
        assert!(matches!(app.top_screen(), Screen::Statistics(_)));
        app.advance_animations(InputClock(3_000));
        assert_eq!(
            app.ui.next_ahead.request(),
            Some(crate::corridor::CorridorKey { filter: PoiCategorySet::only(PoiCategory::Water), anchor_m: 0 }),
            "…and the identical route index re-queries against the new bytes"
        );
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
    // The **pure** hysteresis resolvers (`resolve_active_climb` / `resolve_next_waypoint`) are
    // pinned in `ride_engine.rs`, next to the policy they encode. Here the App-side wiring is
    // driven end-to-end — build-on-load, clear-on-unload, the once-per-entry `ClimbProfile::fill`,
    // and the C5 auto-switch — through `App::update_active_climb` and `App::tick` over the
    // committed `grimsel-climb.obcr` fixture (3 back-to-back climbs).

    use obc_formats::io::SliceSource;
    use obc_route::RouteIndex;

    /// The committed Grimsel fixture bytes (3 back-to-back climbs), embedded so the `no_std` lib
    /// tests need no `std::fs`. Boundaries: 501–11067, 11067–14472, 14472–18547; total ~18.7 km.
    const GRIMSEL: &[u8] = include_bytes!("../../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr");

    /// Parse the fixture into a `RouteIndex` the callers pair with a `SliceSource` over [`GRIMSEL`].
    fn grimsel_index() -> RouteIndex {
        let src = SliceSource(GRIMSEL);
        RouteIndex::read(&src).unwrap()
    }

    /// Route-relative Inspect keeps a distance cursor in the input path, then resolves it to the
    /// streamed route exactly once at the pre-draw seam. That makes Up/Down follow real bends
    /// without giving gesture handling ownership of route I/O.
    #[test]
    fn pan_route_cursor_syncs_camera_to_route_geometry() {
        let idx = grimsel_index();
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);
        let mut state = AppState::new(0, 0, 1.0);
        state.enter_pan(true, 1_000);

        let start = route.position_at(1_000).unwrap();
        state.sync_pan_route(&route);
        assert_eq!((state.cam_lon, state.cam_lat), (start.lon, start.lat), "entry centres the route cursor");

        state.pan_step(1, route.total_distance_m);
        let ahead_m = state.pan.unwrap().route_progress_m;
        assert!(ahead_m > 1_000, "a positive step advances cumulative route distance");
        let ahead = route.position_at(ahead_m).unwrap();
        state.sync_pan_route(&route);
        assert_eq!((state.cam_lon, state.cam_lat), (ahead.lon, ahead.lat), "the camera lands on the curved route");
    }

    fn tick_without_fix(app: &mut App, route: Option<&RouteReader>) {
        let mut loc = OneFix(None);
        app.tick(RideClock(0), Sensors::new(&mut loc), route);
    }

    #[test]
    fn seam_commit_atomically_reanchors_progress_matcher_and_guidance() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let idx = grimsel_index();
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);
        app.activity.active_route = Some(0);
        app.test_start_ride();
        tick_without_fix(&mut app, Some(&route)); // establish route/session caches

        app.activity.progress_m = 1_000;
        let session = app.ride_session();
        let mode = app.activity.mode;
        app.activity.request_seam(0, 12_000); // lands on the fixture's second climb
        tick_without_fix(&mut app, Some(&route));
        assert_eq!(app.activity.progress_m, 12_000);
        assert!(!app.activity.off_route);
        assert_eq!(app.activity.active_climb, Some(1), "climb guidance re-derived at the new anchor");
        assert!(app.activity.pending_seam().is_none());
        assert_eq!(app.ride_session(), session);
        assert_eq!(app.activity.mode, mode);

        // A fix in the skipped stretch cannot pull matching behind the durable floor.
        let p = route.position_at(2_000).unwrap();
        let mut loc = OneFix(Some(Fix { lon: p.lon, lat: p.lat, course: None, speed_mps: None }));
        app.tick(RideClock(1_000), Sensors::new(&mut loc), Some(&route));
        assert!(app.activity.off_route);
        assert_eq!(app.activity.progress_m, 12_000);

        // Removing/reloading the route resets the floor; an early fix on the reloaded geometry can
        // establish an early first lock again (the tracking session itself is still the same).
        app.activity.active_route = None;
        tick_without_fix(&mut app, None);
        app.activity.active_route = Some(0);
        tick_without_fix(&mut app, Some(&route));
        let mut loc = OneFix(Some(Fix { lon: p.lon, lat: p.lat, course: None, speed_mps: None }));
        app.tick(RideClock(2_000), Sensors::new(&mut loc), Some(&route));
        assert!(!app.activity.off_route);
        assert!(app.activity.progress_m < 3_000, "route reload cleared the old 12 km floor");

        // A new session on the same route also clears every seam-derived anchor.
        app.activity.request_seam(0, 8_000);
        tick_without_fix(&mut app, Some(&route));
        assert_eq!(app.activity.progress_m, 8_000);
        app.test_end_ride();
        app.test_start_ride();
        tick_without_fix(&mut app, Some(&route));
        assert_eq!(app.activity.progress_m, 0);
        assert!(!app.activity.off_route);
    }

    #[test]
    fn failed_seam_seek_keeps_old_anchor_and_retries() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let idx = grimsel_index();
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);
        app.activity.active_route = Some(0);
        app.test_start_ride();
        tick_without_fix(&mut app, Some(&route));
        app.activity.progress_m = 1_000;
        app.activity.request_seam(0, 4_000);

        let empty = SliceSource(&[]);
        let unreadable = RouteReader::new(&idx, &empty);
        tick_without_fix(&mut app, Some(&unreadable));
        assert_eq!(app.activity.progress_m, 1_000, "failed decode does not split Activity from matcher");
        assert!(app.activity.pending_seam().is_some(), "transient failure remains retryable");

        tick_without_fix(&mut app, Some(&route));
        assert_eq!(app.activity.progress_m, 4_000);
        assert!(app.activity.pending_seam().is_none());
    }

    fn app_with_detour_chooser_on_beta() -> App {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.state.has_nav_graph = true;
        app.state.user_fix = Some(Fix { lon: 7_800_000, lat: 48_000_000, course: None, speed_mps: None });
        app.set_routes_with_ids(&[summary("Alpha"), summary("Beta"), summary("Gamma")], &[10, 20, 30]);
        app.activity.active_route = Some(1);
        app.activity.progress_m = 1_000;
        app.activity.route_total_m = 5_000;
        app.test_start_ride();
        let chooser = crate::screen::DetourScreen::new(&app.activity);
        *app.ui.stack.last_mut().unwrap() = Screen::Detour(chooser);
        app
    }

    #[test]
    fn detour_chooser_and_queued_plan_follow_route_identity_across_rescans() {
        let mut app = app_with_detour_chooser_on_beta(); // Beta id 20 at index 1
        app.set_routes_with_ids(&[summary("Gamma"), summary("Alpha"), summary("Beta")], &[30, 10, 20]);
        assert_eq!(app.activity.active_route, Some(2), "active navigation followed Beta to index 2");

        app.apply_gesture(Gesture::Press);
        assert_eq!(app.navigator.pending_detour_request().unwrap().route, 2, "the open chooser followed Beta too");

        // Before the host drains the request, another rescan moves Beta again.
        app.set_routes_with_ids(&[summary("Beta"), summary("Gamma"), summary("Alpha")], &[20, 30, 10]);
        assert_eq!(app.activity.active_route, Some(0));
        assert_eq!(app.navigator.pending_detour_request().unwrap().route, 0, "the queued plan request follows Beta");
    }

    #[test]
    fn vanished_detour_route_disables_the_chooser_and_clears_a_queued_plan() {
        let mut open = app_with_detour_chooser_on_beta();
        open.set_routes_with_ids(&[summary("Alpha"), summary("Gamma")], &[10, 30]);
        assert_eq!(open.activity.active_route, None, "vanished Beta unloads navigation");
        open.apply_gesture(Gesture::Press);
        assert!(matches!(open.top_screen(), Screen::Detour(_)), "an unavailable chooser stays safely cancellable");
        assert!(open.navigator.pending_detour_request().is_none(), "it never retargets the route now at old index 1");

        let mut queued = app_with_detour_chooser_on_beta();
        queued.apply_gesture(Gesture::Press);
        assert!(queued.navigator.pending_detour_request().is_some());
        queued.set_routes_with_ids(&[summary("Alpha"), summary("Gamma")], &[10, 30]);
        assert!(queued.navigator.pending_detour_request().is_none(), "a queued plan for vanished Beta is cancelled");
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
        app.ride.climbs = route.detect_climbs();
        assert_eq!(app.ride.climbs.len(), 3, "the Grimsel fixture segments into 3 climbs");

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
        assert_eq!(app.ride.climb_fill_count, 3, "the detail buffer is rebuilt exactly on the 3 entries, not per fix");
    }

    /// Off-route freezes the active climb: a stale (frozen) match must not strand the rider onto a
    /// climb, nor drop the one they were on — the state holds until they rejoin and progress moves.
    #[test]
    fn update_active_climb_freezes_while_off_route() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let idx = grimsel_index();
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);
        app.ride.climbs = route.detect_climbs();

        // On climb 0 (progress mid-first-climb).
        app.activity.progress_m = 5000;
        app.update_active_climb(&route);
        assert_eq!(app.activity.active_climb, Some(0));
        let fills_on_climb = app.ride.climb_fill_count;

        // Go off-route: progress freezes (the matcher holds it). Even a progress value that would
        // otherwise be past every climb must not change the active climb while off-route.
        app.activity.off_route = true;
        app.activity.progress_m = 99_999;
        app.update_active_climb(&route);
        assert_eq!(app.activity.active_climb, Some(0), "off-route holds the current climb");
        assert_eq!(app.ride.climb_fill_count, fills_on_climb, "no refill while off-route");
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
            app.ride.climbs = route.detect_climbs();
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
        let _ = app.ui.stack.push(Screen::Menu(MenuScreen::new()));
        assert_ne!(app.top_screen().kind(), ScreenKind::Riding, "top is now a menu, not a riding view");
        enter_first_climb(&mut app, &idx);
        assert!(matches!(app.top_screen(), Screen::Menu(_)), "the menu is left untouched by the entry edge");
        // And the map underneath it is still the Map — the switch didn't reach past the menu.
        assert!(
            matches!(app.ui.stack[app.ui.stack.len() - 2], Screen::Map(_)),
            "the base riding view is untouched too"
        );
    }

    /// The rider pulls the card while the System screen is up. The board answers its next scan with
    /// `CardScanned { free_bytes: None }`, and the row goes back to `--` rather than keeping the
    /// byte count it read off a card that is no longer in the device.
    ///
    /// This is the one path the legacy protocol actually produces: `ride.rs`'s producer yields
    /// `None` for no mounted medium *and* for no FSInfo free count, and it has always blanked.
    #[test]
    fn a_card_scan_with_no_figure_blanks_the_free_space_row() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.storage.note_measured(Some(8 * 1024 * 1024));
        assert_eq!(app.storage.free_bytes(), Some(8 * 1024 * 1024), "the scan answered");

        app.storage.note_measured(None);
        assert_eq!(app.storage.free_bytes(), None, "and a scan with no figure leaves the rider a `--`");
    }

    /// The preview polyline is *derived* from the detour plan, so Back on the preview takes it with
    /// the plan it previewed. It is drawn over the still-active route: a shape that outlived its
    /// detour is a line to nowhere, and the rider would be looking at a turn nobody is going to
    /// make.
    #[test]
    fn cancelling_a_detour_drops_its_preview_polyline() {
        use crate::screen::{DetourPreviewScreen, DetourScreen};
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.set_routes_with_ids(&[summary("Road")], &[7]);
        app.state.has_nav_graph = true;
        app.state.user_fix = Some(Fix { lon: 7_800_000, lat: 48_000_000, course: None, speed_mps: None });
        app.activity.active_route = Some(0);
        app.activity.progress_m = 1_000;
        app.activity.route_total_m = 20_000;
        app.test_start_ride();

        // Plan a detour and land its preview, exactly as the flow does.
        let chooser = DetourScreen::new(&app.activity);
        let preview =
            crate::host::DetourPreview { cost_delta_m: 420, total_distance_m: 1_220, rejoin_m: 2_000, ascent_m: None };
        app.admit_navigator_intent(NavigatorIntent::PlanDetour(crate::activity::DetourRequest {
            route: 0,
            from: (7_800_000, 48_000_000),
            progress_m: 1_000,
            target_m: 1_800,
        }));
        let _ = app.ui.stack.push(Screen::Detour(chooser));
        let _ = app.ui.stack.push(Screen::DetourPreview(DetourPreviewScreen::new(&chooser, preview)));
        app.set_detour_preview(&[(7_812_000, 48_001_000), (7_816_000, 48_001_000)]);
        assert!(!app.catalogs.detour_preview_for(Some(0)).is_empty(), "the host's shape is cached");

        app.apply_gesture(Gesture::Back); // the rider drops the detour
        assert!(
            app.catalogs.detour_preview_for(Some(0)).is_empty(),
            "and the shape goes with the plan, not one frame later"
        );
    }

    /// The Detour chooser is map-backed and live, but it is an interaction in progress rather
    /// than an auto-switch sibling. A climb entry must preserve both the chooser and its
    /// selected distance.
    #[test]
    fn auto_never_switches_away_from_the_detour_chooser() {
        use crate::screen::DetourScreen;
        use crate::settings::ClimbMode;
        let (mut app, idx) = climb_app(ClimbMode::Auto);
        app.state.has_nav_graph = true;
        app.state.user_fix = Some(Fix { lon: 7_800_000, lat: 48_000_000, course: None, speed_mps: None });
        app.activity.active_route = Some(0);
        app.activity.progress_m = 1_000;
        app.activity.route_total_m = 20_000;
        app.test_start_ride();
        let chooser = DetourScreen::new(&app.activity);
        *app.ui.stack.last_mut().unwrap() = Screen::Detour(chooser);
        app.apply_gesture(Gesture::Step(2)); // selected distance = the 600 m minimum + 200 m

        enter_first_climb(&mut app, &idx); // live anchor advances to 5 km on climb entry
        assert!(matches!(app.top_screen(), Screen::Detour(_)), "climb entry preserves the open chooser");

        app.apply_gesture(Gesture::Press);
        let req = app.navigator.pending_detour_request().expect("the preserved chooser still plans");
        assert_eq!((req.route, req.target_m), (0, 5_800), "the 800 m selection survives the climb edge");
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

    /// If ride chrome was opened from Climb, the crest repairs that hidden caller without
    /// dismissing the interaction on top. Back from the Detour chooser must reveal Map, never
    /// No climb.
    #[test]
    fn crest_repairs_a_hidden_climb_below_the_detour_chooser() {
        use crate::settings::ClimbMode;
        let (mut app, idx) = climb_app(ClimbMode::Auto);
        app.state.has_nav_graph = true;
        // The ride opens first: a session start zeroes the ride, and this trace is about what the
        // *crest* does to a hidden caller.
        app.test_start_ride();
        enter_first_climb(&mut app, &idx); // top = Climb
        app.activity.active_route = Some(0);
        app.activity.route_total_m = 50_000;

        app.apply_gesture(Gesture::BackHold); // [Home, Climb, RideMenu]
        app.apply_gesture(Gesture::Step(1));
        app.apply_gesture(Gesture::Press); // RideMenu Replace → [Home, Climb, Detour]
        assert!(matches!(app.top_screen(), Screen::Detour(_)));

        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);
        app.activity.progress_m = 50_000;
        app.update_active_climb(&route);
        assert_eq!(app.activity.active_climb, None);
        assert!(matches!(app.top_screen(), Screen::Detour(_)), "crest does not dismiss the chooser");
        assert!(
            matches!(app.ui.stack[app.ui.stack.len() - 2], Screen::Map(_)),
            "the hidden Climb caller is repaired in place"
        );

        app.apply_gesture(Gesture::Back);
        assert!(matches!(app.top_screen(), Screen::Map(_)), "Back reveals the repaired riding caller");
    }

    /// The crest return only repairs a Climb screen: if the rider is on some other view and no
    /// Climb caller exists when the climb ends, that view is left as-is (never force-switched).
    #[test]
    fn crest_leaves_other_screens_untouched() {
        use crate::screen::MenuScreen;
        use crate::settings::ClimbMode;
        let (mut app, idx) = climb_app(ClimbMode::Manual); // Manual: entry won't switch
        enter_first_climb(&mut app, &idx);
        let _ = app.ui.stack.push(Screen::Menu(MenuScreen::new())); // now on a menu, mid-climb
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
            app.tick(RideClock(0), Sensors::new(&mut loc), route);
        };
        no_loc(&mut app, Some(&route));
        assert!(app.ride.climbs.is_empty(), "no active route → no climbs, even with a reader present");
        assert!(app.ride.climbs_route.is_none());
        assert!(
            app.ride.waypoints.is_empty() && app.ride.waypoints_route.is_none(),
            "no active route → no waypoint table"
        );
        assert_eq!(app.activity.waypoint_count, 0, "the gesture-side table length mirrors the empty cache");

        // Load the route (active_route = Some) and tick with the reader → climbs segmented once, and
        // the waypoint table loaded on the same edge (GRIMSEL carries none, so the table is empty but
        // the build key advances to Some(0) — the load ran).
        app.activity.active_route = Some(0);
        no_loc(&mut app, Some(&route));
        assert_eq!(app.ride.climbs.len(), 3, "an active route + reader segments the climbs on load");
        assert_eq!(app.ride.climbs_route, Some(0));
        assert_eq!(app.ride.waypoints_route, Some(0), "the waypoint table loads on the same route edge");
        assert_eq!(
            app.activity.waypoint_count,
            app.ride.waypoints.len(),
            "the gesture-side table length mirrors the loaded resident cache"
        );

        // Unload (active_route → None) and tick → the climbs / waypoints and their derived indices clear.
        app.activity.active_climb = Some(0); // pretend we were on a climb
        app.activity.next_waypoint = Some(0); // …and had a next waypoint
        app.activity.active_route = None;
        no_loc(&mut app, None);
        assert!(app.ride.climbs.is_empty(), "unloading the route clears the climbs");
        assert!(app.ride.climbs_route.is_none());
        assert_eq!(app.activity.active_climb, None, "and the on-climb state is dropped");
        assert!(
            app.ride.waypoints.is_empty() && app.ride.waypoints_route.is_none(),
            "unloading clears the waypoint table"
        );
        assert_eq!(app.activity.next_waypoint, None, "and the next-waypoint index is dropped");
        assert_eq!(app.activity.waypoint_count, 0, "and the gesture-side table length clears with it");
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
        let _ = app.ui.stack.push(Screen::Menu(MenuScreen::new()));
        let _ = app.ui.stack.push(Screen::Settings(SettingsScreen::new()));
        app.ui.last_input_ms = 0;

        idle_tick(&mut app, 29_000); // still inside the window
        assert!(matches!(app.top_screen(), Screen::Settings(_)), "no return before the deadline");

        idle_tick(&mut app, 30_000); // deadline reached
        assert_eq!(app.ui.stack.len(), 1, "cleared to the Home root");
        assert!(matches!(app.top_screen(), Screen::Home(_)), "and the top is Home");
    }

    /// Returning to Home reseeds the screensaver backdrop, exactly as a manual return does.
    #[test]
    fn idle_return_home_reseeds_the_backdrop() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.settings.idle_return = IdleReturn::S15;
        let _ = app.ui.stack.push(Screen::Menu(MenuScreen::new()));
        app.ui.last_input_ms = 0;
        idle_tick(&mut app, 20_000);
        let Some(Screen::Home(home)) = app.ui.stack.first() else { panic!("back on Home") };
        assert_eq!(home.backdrop_seed(), 20_000, "the backdrop reseeds to the return's clock");
    }

    /// Tracking: a menu screen returns to the Map; the deliberate ride views do not time out.
    #[test]
    fn idle_returns_to_map_when_tracking_from_a_menu() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map], Riding
        app.test_start_ride(); // arm a tracking session
        app.settings.idle_return = IdleReturn::S30;
        let _ = app.ui.stack.push(Screen::Menu(MenuScreen::new()));
        app.ui.last_input_ms = 0;

        idle_tick(&mut app, 30_000);
        assert!(matches!(app.top_screen(), Screen::Map(_)), "a menu times out to the Map mid-ride");
        assert_eq!(app.ui.stack.len(), 2, "landed on [Home, Map], not deeper");
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
            app.test_start_ride();
            app.settings.idle_return = IdleReturn::S15;
            *app.ui.stack.last_mut().unwrap() = view; // replace the base Map with the view under test
            let kind_before = core::mem::discriminant(app.top_screen());
            app.ui.last_input_ms = 0;
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
            // The passkey card is level-driven: raise the level it stands for, or the card
            // scheduler's per-pass sweep would (correctly) take a card with no passkey behind it
            // straight back off the stack.
            if matches!(card, Screen::Passkey(_)) {
                app.ui.cards.set_passkey(Some(123_456));
            }
            let _ = app.ui.stack.push(card);
            app.ui.last_input_ms = 0;
            idle_tick(&mut app, 20_000);
            assert_eq!(core::mem::discriminant(app.top_screen()), kind, "the modal card stays up");
        }
    }

    /// Time spent behind an idle-exempt wait is not banked against the screen that follows it.
    /// This is the #859 failure mode: a plan taking longer than the timeout used to reveal the
    /// overview and have it swept Home in the same pass.
    #[test]
    fn idle_exemption_suspends_then_restarts_the_clock() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.settings.idle_return = IdleReturn::S15;
        let _ = app.ui.stack.push(Screen::NavPlanning(NavPlanningScreen::new("Slow route")));
        app.ui.last_input_ms = 0;

        idle_tick(&mut app, 120_000);
        assert!(matches!(app.top_screen(), Screen::NavPlanning(_)), "the slow plan stays visible");
        assert!(!app.ui.idle_return_timing, "the exempt screen suspended, rather than aged, the clock");

        *app.ui.stack.last_mut().unwrap() =
            Screen::RouteOverview(crate::screen::RouteOverviewScreen::computed(0, None));
        idle_tick(&mut app, 120_000);
        assert!(matches!(app.top_screen(), Screen::RouteOverview(_)), "completion receives a fresh window");
        assert_eq!(app.ui.last_input_ms, 120_000, "the ordinary screen starts a new idle window");
        assert_eq!(app.ms_until_next_wake(120_000), Some(15_000), "the full timeout is armed");

        idle_tick(&mut app, 134_999);
        assert!(matches!(app.top_screen(), Screen::RouteOverview(_)), "the overview keeps the whole window");
        idle_tick(&mut app, 135_000);
        assert!(matches!(app.top_screen(), Screen::Home(_)), "the restarted timeout eventually fires");
    }

    /// The route-less **browse map** (Map on top, not tracking — Menu → Map) is a deliberate view,
    /// so it's exempt from the idle-return timeout even though it isn't the Home root: elapse well
    /// past the deadline and it stays put (unlike a menu, which would return to Home).
    #[test]
    fn browse_map_is_exempt_from_idle_return() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0)); // Idle, not tracking
        app.settings.idle_return = IdleReturn::S15;
        let _ = app.ui.stack.push(Screen::Map(MapScreen::new())); // the browse map over Home
        app.ui.last_input_ms = 0;
        idle_tick(&mut app, 60_000);
        assert!(matches!(app.top_screen(), Screen::Map(_)), "the browse map is a deliberate view — never yanked");
        // The browse map's only pending wake is the one-shot start hint's auto-hide (T6, #684); once
        // that window has elapsed it arms no wake at all — in particular no idle-return wake.
        idle_tick(&mut app, 60_000 + 4_000);
        assert_eq!(app.ms_until_next_wake(60_000 + 4_000), None, "and it arms no idle wake");

        // A menu over Home, by contrast, does return.
        *app.ui.stack.last_mut().unwrap() = Screen::Menu(MenuScreen::new());
        app.ui.last_input_ms = 60_000;
        app.ui.idle_return_timing = true; // model the gesture that opened the menu
        idle_tick(&mut app, 120_000);
        assert!(matches!(app.top_screen(), Screen::Home(_)), "a menu still returns to Home on the timeout");
    }

    /// Any gesture resets the idle deadline — a step 1 ms before it would fire buys another full window.
    #[test]
    fn a_gesture_resets_the_idle_deadline() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.settings.idle_return = IdleReturn::S30;
        let _ = app.ui.stack.push(Screen::Menu(MenuScreen::new()));
        app.ui.last_input_ms = 0;

        // A gesture at 29 s (just shy of the deadline) resets the clock.
        app.ui.now_ms = 29_000;
        app.apply_gesture(Gesture::Step(1));
        assert_eq!(app.ui.last_input_ms, 29_000, "the gesture reset the idle clock");

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
        let _ = app.ui.stack.push(Screen::Menu(MenuScreen::new()));
        app.ui.last_input_ms = 0;
        idle_tick(&mut app, 10 * 60_000); // ten minutes
        assert!(matches!(app.top_screen(), Screen::Menu(_)), "Never never returns");
        assert_eq!(app.ms_until_next_wake(10 * 60_000), None, "and arms no idle wake");
    }

    /// The idle deadline is folded into the host's wake so a parked device wakes to return.
    #[test]
    fn idle_return_arms_a_wake() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.settings.idle_return = IdleReturn::S30;
        let _ = app.ui.stack.push(Screen::Menu(MenuScreen::new()));
        app.ui.last_input_ms = 0;
        idle_tick(&mut app, 10_000);
        assert_eq!(app.ms_until_next_wake(10_000), Some(20_000), "wake armed 20 s out (30 s − 10 s elapsed)");
    }

    /// The DFU install request (epic #615 S4) drains exactly once — the create-route request
    /// contract. `request_dfu_install` (the `dfu-install` debug path) posts the
    /// [`DfuAction::Install`] the board's drain matches on. The boot-update verdict's own
    /// once-only rule lives with the card scheduler that consumes it.
    #[test]
    fn dfu_install_request_is_take_once() {
        use crate::activity::DfuAction;
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        assert_eq!(drain_dfu(&mut app), None, "nothing pending at boot");
        app.dfu.admit_intent(crate::dfu::DfuIntent::InstallRequested);
        assert_eq!(drain_dfu(&mut app), Some(DfuAction::Install), "the posted request drains");
        assert_eq!(drain_dfu(&mut app), None, "…exactly once");
    }

    /// The S6 remote-check seam (epic #615 S6, #621): a BLE `installFw` opens the **same** scan →
    /// confirm flow the System menu's press does — push the DfuCheck wait + post
    /// [`DfuAction::Scan`], never `Install` — exactly once per accepted call.
    #[test]
    fn remote_dfu_check_opens_scan_flow_once() {
        use crate::activity::DfuAction;
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        assert!(app.open_remote_dfu_check(), "an idle app opens the flow");
        let checks = app.ui.stack.iter().filter(|s| matches!(s, Screen::DfuCheck(_))).count();
        assert_eq!(checks, 1, "exactly one wait screen pushed");
        assert_eq!(drain_dfu(&mut app), Some(DfuAction::Scan), "a Scan is posted — NEVER Install");
        assert_eq!(drain_dfu(&mut app), None, "…exactly once");
    }

    /// Remote-check deferral behind the passkey card (S6, #621): the request is *deferred*, not
    /// dropped — `open_remote_dfu_check` returns `false` (the board keeps its pending flag and
    /// retries), posts nothing, pushes nothing; once the card clears, the same call opens the flow.
    #[test]
    fn remote_dfu_check_defers_behind_the_passkey_card() {
        use crate::activity::DfuAction;
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let _ = app.ui.stack.push(Screen::Passkey(crate::screen::PasskeyScreen::new(123_456)));
        assert!(!app.open_remote_dfu_check(), "deferred while the pairing code shows");
        assert!(!app.ui.stack.iter().any(|s| matches!(s, Screen::DfuCheck(_))), "nothing pushed");
        assert_eq!(drain_dfu(&mut app), None, "nothing posted");
        // The card clears (pairing completed/failed) → the retried drain opens the flow.
        app.ui.stack.pop();
        assert!(app.open_remote_dfu_check(), "opens once the card cleared");
        assert!(matches!(app.top_screen(), Screen::DfuCheck(_)));
        assert_eq!(drain_dfu(&mut app), Some(DfuAction::Scan));
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
        assert_eq!(drain_dfu(&mut app), Some(DfuAction::Scan), "the one Scan");
        assert!(!app.open_remote_dfu_check(), "wait screen still up ⇒ still deferred");
        assert_eq!(app.ui.stack.iter().filter(|s| matches!(s, Screen::DfuCheck(_))).count(), 1);

        // The rider's own confirm screen (menu-opened flow) blocks a remote open the same way.
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let mk = |v: &str| {
            let mut s = heapless::String::new();
            let _ = s.push_str(v);
            s
        };
        let report = crate::dfu::DfuScanReport { installed: mk("v1"), staged: mk("v2"), first_install: false };
        let _ = app.ui.stack.push(Screen::DfuConfirm(crate::screen::DfuConfirmScreen::new(report)));
        assert!(!app.open_remote_dfu_check(), "a confirm on the stack ⇒ deferred, never yanked");
        assert_eq!(drain_dfu(&mut app), None);

        // Recording defers (the arm ends in a reboot — a live ride would be lost).
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.test_start_ride();
        assert!(!app.open_remote_dfu_check(), "deferred while recording");
        assert_eq!(drain_dfu(&mut app), None);
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
            synced_at_utc: 0,
        };
        app.set_rides(&[ride("A"), ride("B")], &[7, 9]);

        assert_eq!(ride_track_request(&app), None, "no detail open — no request");

        app.activity.viewed_ride = Some(1); // the Rides press's entry side-effect
        assert_eq!(ride_track_request(&app), Some(9), "the viewed ride's durable id");
        assert_eq!(ride_track_request(&app), Some(9), "re-polls until the host answers");

        // A failed stream still answers — no per-pass grind.
        let key = app.derived_needs().ride_track.expect("the open detail wants its track");
        app.apply_derived(DerivedInputs::ride_track(DerivedInput::failed(key)), DerivedTargets::NONE);
        assert_eq!(ride_track_request(&app), None, "answered for this ride");

        // A rescan drops ride A: id 9 moves to index 0. The viewed key and the answer key both
        // follow by identity, so nothing re-fires.
        app.set_rides(&[ride("B")], &[9]);
        assert_eq!(app.activity.viewed_ride, Some(0), "the viewed index follows the id");
        assert_eq!(ride_track_request(&app), None, "the answer moved with it");

        // The viewed ride itself vanishing clears the keys — nothing left to request.
        app.set_rides(&[ride("A")], &[7]);
        assert_eq!(app.activity.viewed_ride, None);
        assert_eq!(ride_track_request(&app), None);
    }

    // ==================== The typed host protocol (FAR-07, #800) ====================

    fn summary(name: &str) -> RouteSummary {
        let mut n = heapless::String::<48>::new();
        let _ = n.push_str(name);
        RouteSummary {
            name: n,
            distance_km: 10,
            climb_m: 100,
            bbox: obc_map_scene::BBox { min_lon: 0, min_lat: 0, max_lon: 1000, max_lat: 1000 },
            start_lon: 100,
            start_lat: 100,
        }
    }

    fn ride_summary(name: &str) -> crate::ride::RideSummary {
        crate::ride::RideSummary {
            name: heapless::String::try_from(name).unwrap(),
            start_time: 1_720_000_000,
            distance_m: 1_000,
            moving_time_s: 600,
            climb_m: 10,
            synced: false,
            synced_at_utc: 0,
        }
    }

    /// The residual drains in its class order, exactly once each, and reaches nothing else.
    ///
    /// Every other class the mailbox once carried is a domain's now: they are posted here too, and
    /// the drain must walk straight past them. That is the property PR #1505's regression turned on
    /// — a walk that *pulled* from each domain would mint a planner operation nobody answers.
    #[test]
    fn the_residual_drains_in_class_order_and_reaches_nothing_else() {
        use crate::activity::NavRequest;
        use crate::host::{DrainStatus, HostCommand, HostMailbox};

        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.stamp_clock(DateTime { year: 2026, month: 7, day: 1, hour: 12, minute: 0 }, 0, None, ClockTrust::Gps);
        app.set_routes_with_ids(&[summary("Alpha"), summary("Beta")], &[10, 11]);
        app.set_rides(&[ride_summary("R")], &[7]);

        // The one residual class…
        app.state.ble_forget_pending = true;
        // …and a request from every domain that owns its own lifecycle now.
        app.activity.request_trip_delete(42);
        app.storage.admit_intent(crate::device_core::storage_info::StorageInfoIntent::RefreshRequested);
        app.admit_navigator_intent(NavigatorIntent::PlanRoute(NavRequest::new((0, 0), (500, 500), "To the col")));
        app.dfu.admit_intent(crate::dfu::DfuIntent::ScanRequested);
        app.activity.request_route_delete(1);
        app.activity.request_ride_delete(0);
        app.arm_settings_save();
        app.retention.test_push(crate::retention::SweepAction::StampRoute(10));

        let mut mailbox: HostMailbox = HostMailbox::new();
        assert_eq!(app.drain_residual_commands(&mut mailbox), DrainStatus::Complete);
        let mut drained: heapless::Vec<HostCommand, 4> = heapless::Vec::new();
        while let Some(cmd) = mailbox.pop() {
            let _ = drained.push(cmd);
        }
        assert!(matches!(drained.as_slice(), [HostCommand::ForgetBond]), "one class, and nothing else: {drained:?}");

        // The domains it walked past still hold their work, untouched.
        assert!(drain_nav(&mut app).is_some(), "the planner request is still Navigator's to hand out");
        assert_eq!(drain_dfu(&mut app), Some(crate::activity::DfuAction::Scan));
        assert!(drain_persist(&mut app).is_some());
        assert!(app.retention.has(crate::retention::SweepKind::StampRoute), "and the sweep's stamp is retention's");
        assert!(app.activity.take_route_delete().is_some(), "and the rider's route delete is the catalog's to take");
        assert_eq!(app.activity.take_trip_delete(), Some(42), "and the trip cascade is the catalog's too");

        // And it is a one-shot the drain clears.
        assert_eq!(app.drain_residual_commands(&mut mailbox), DrainStatus::Complete);
        assert!(mailbox.is_empty(), "nothing pending, nothing drained");
    }

    /// The saturation policy is backpressure, never loss: a drain into a mailbox without room
    /// consumes nothing for the class it can't hand over — it stays latched, is reported by
    /// `MailboxFull`, and comes out once the host makes room.
    #[test]
    fn full_mailbox_backpressures_without_losing_commands() {
        use crate::host::{DrainStatus, HostCommand, HostMailbox};

        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        // The one slot is already taken, so the residual command cannot be handed over.
        let mut mailbox: HostMailbox<1> = HostMailbox::new();
        assert!(mailbox.push(HostCommand::ForgetBond));
        app.state.ble_forget_pending = true;

        assert_eq!(app.drain_residual_commands(&mut mailbox), DrainStatus::MailboxFull);
        assert!(app.state.ble_forget_pending, "the command stays latched — never silently dropped");

        // The host makes room → the latched command drains intact.
        assert_eq!(mailbox.pop(), Some(HostCommand::ForgetBond));
        assert_eq!(app.drain_residual_commands(&mut mailbox), DrainStatus::Complete);
        assert_eq!(mailbox.pop(), Some(HostCommand::ForgetBond));
        assert!(!app.state.ble_forget_pending);
    }

    /// The DFU slot is most-recent-wins **by design** (one phase in flight; a later rider post
    /// supersedes) — encoded here rather than inherited from `Option` replacement.
    #[test]
    fn dfu_slot_is_most_recent_wins() {
        use crate::activity::DfuAction;
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.dfu.admit_intent(crate::dfu::DfuIntent::ScanRequested);
        app.dfu.admit_intent(crate::dfu::DfuIntent::InstallRequested);
        assert_eq!(drain_dfu(&mut app), Some(DfuAction::Install), "the later phase superseded");
        assert_eq!(drain_dfu(&mut app), None);
    }

    /// The settings write stays gated on leaving the settings subtree — a dirty value under an open
    /// settings screen is not yet work for the executor.
    #[test]
    fn persist_settings_waits_for_subtree_exit_and_is_single_sourced() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let _ = app.ui.stack.push(Screen::Settings(crate::screen::SettingsScreen::new()));
        app.arm_settings_save(); // rev → 1
        assert!(!settings_dirty(&mut app), "still editing — nothing owed yet");

        app.ui.stack.pop(); // leave the subtree
        assert_eq!(drain_persist(&mut app), Some(1));
        assert!(!settings_dirty(&mut app), "one emit, then Awaiting — no second pending state");
    }

    // ==================== #810: acknowledged, retryable settings persistence ====================
    //
    // These drive the revision handshake through the domain's own seam: `SettingsMachine` hands out
    // one write and validates the answer's operation token and revision independently.

    /// A settings save on a settings-screen edit stays held until the rider leaves the subtree, then
    /// emits exactly once per sweep regardless of how many steps changed the value — no per-step
    /// RRAM write, and none while any settings screen is on top (the mandatory "no writes during a
    /// stepper sweep / inside the subtree" case).
    #[test]
    fn no_persist_during_a_stepper_sweep_inside_the_subtree() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let _ = app.ui.stack.push(Screen::Settings(crate::screen::SettingsScreen::new()));
        // A sweep of edits while inside the subtree: several revisions, but never an emit.
        for _ in 0..5 {
            app.arm_settings_save();
            assert_eq!(drain_persist(&mut app), None, "held while a settings screen is on top");
        }
        app.ui.stack.pop(); // leave the subtree
        assert_eq!(drain_persist(&mut app), Some(5), "one coalesced emit for the latest revision");
        assert_eq!(drain_persist(&mut app), None, "and only once — now Awaiting the ack");
    }

    /// Success: the emitted revision's ack clears the dirty state, and nothing re-emits afterward.
    #[test]
    fn persist_success_clears_the_dirty_state() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let mut host = SettingsHost::default();
        app.arm_settings_save();
        assert_eq!(host.drain(&mut app), Some(1));
        host.ack(&mut app, 1);
        assert_eq!(host.drain(&mut app), None, "acked → Clean, nothing owed");
    }

    /// A failed write does **not** lose the dirty state (the exact #810 bug): it re-arms a bounded
    /// backoff, holds off within the window, then re-emits the *same* revision once the window passes.
    #[test]
    fn transient_failure_then_retry_keeps_the_revision() {
        use crate::screen::WarningFlags;
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let mut host = SettingsHost::default();
        app.ui.now_ms = 10_000;
        app.arm_settings_save();
        assert_eq!(host.drain(&mut app), Some(1));
        host.fail(&mut app, 1);
        // Failure is observable on the advisory card, not just logged.
        assert!(
            app.ui
                .stack
                .iter()
                .any(|s| matches!(s, Screen::Warning(w) if w.flags().contains(WarningFlags::SETTINGS_ERROR))),
            "a failed persist raises the settings advisory",
        );
        assert_eq!(host.drain(&mut app), None, "inside the backoff window — no retry yet");
        app.ui.now_ms += SETTINGS_RETRY_BACKOFF_MS; // window elapsed
        assert_eq!(host.drain(&mut app), Some(1), "the same revision is retried, not lost");
        host.ack(&mut app, 1);
        assert_eq!(host.drain(&mut app), None, "the retry's ack finally clears it");
    }

    /// Repeated failure paces retries: exactly one emit per backoff window, never a per-pass storm of
    /// RRAM writes while the store keeps rejecting.
    #[test]
    fn repeated_failure_is_paced_by_the_backoff_window() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let mut host = SettingsHost::default();
        app.ui.now_ms = 1_000;
        app.arm_settings_save();
        for round in 0..3 {
            assert_eq!(host.drain(&mut app), Some(1), "one emit at the start of round {round}");
            host.fail(&mut app, 1);
            // Several passes inside the window yield nothing — the pacing guard.
            for _ in 0..4 {
                app.ui.now_ms += 100;
                assert_eq!(host.drain(&mut app), None, "no re-emit inside the backoff window");
            }
            app.ui.now_ms += SETTINGS_RETRY_BACKOFF_MS; // cross into the next window
        }
    }

    /// An edit while a save is pending bumps the revision and supersedes it: the stale ack for the old
    /// revision must NOT clear the newer dirty state; the newer revision then persists.
    #[test]
    fn newer_edit_supersedes_and_a_stale_ack_is_ignored() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let mut host = SettingsHost::default();
        app.arm_settings_save(); // rev 1
        assert_eq!(host.drain(&mut app), Some(1)); // Awaiting(1)
        app.arm_settings_save(); // a fresh edit while pending → rev 2, Dirty
                                 // The old save's ack lands late — it is for a superseded revision and must be ignored.
        host.ack(&mut app, 1);
        assert_eq!(host.drain(&mut app), Some(2), "the newer revision still needs persisting");
        host.ack(&mut app, 2);
        assert_eq!(host.drain(&mut app), None, "only the latest ack clears it");
    }

    /// BLE merge under a pending device edit: `merge_ble_settings` adopts the phone's owned fields
    /// without dropping the pending save, so neither the phone's write nor the rider's edit is lost.
    #[test]
    fn ble_merge_under_a_pending_save_loses_neither_side() {
        use crate::settings::Units;
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let mut host = SettingsHost::default();
        app.set_settings(Settings::default()); // seeded Clean

        // A device-only edit is pending (a fix-interval change), not yet persisted.
        app.settings.fix_interval_s = 9;
        app.arm_settings_save(); // rev 1, Dirty

        // The phone writes units=Imperial (persisted to the store by the BLE plane already); the ride
        // loop merges the BLE-owned fields into the live copy.
        app.merge_ble_settings(&Settings { units: Units::Imperial, ..Settings::default() });

        assert_eq!(app.settings().units, Units::Imperial, "the phone's units are adopted");
        assert_eq!(app.settings().fix_interval_s, 9, "the pending device edit is untouched");
        // The save still fires and writes the merged blob — neither side lost.
        assert_eq!(host.drain(&mut app), Some(1), "the pending save survives the BLE merge");

        // The clean-case twin: a BLE merge with nothing pending adds no redundant write.
        host.ack(&mut app, 1);
        app.merge_ble_settings(&Settings { units: Units::Metric, ..Settings::default() });
        assert_eq!(host.drain(&mut app), None, "BLE fields are already persisted — no re-write owed");
    }

    /// Reboot-load fallback: seeding the boot value from the store (or the default when the store is
    /// blank/corrupt) resets the handshake to Clean — a fresh boot never spuriously re-persists.
    #[test]
    fn reboot_load_seeds_clean() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.arm_settings_save(); // pretend a stale dirty state survived somehow
        app.set_settings(Settings::default()); // boot seed (store load or default)
        assert_eq!(drain_persist(&mut app), None, "a seeded boot value is already persisted");
    }

    /// Same-batch confirm→Back (review F1): a cancel posted while the plan request is still
    /// undrained **annihilates** it — the rider's net intent is "no plan", matching what both
    /// per-class and whole-mailbox drains observe, so the host cannot execute a dismissed plan.
    #[test]
    fn a_cancel_annihilates_an_undrained_plan_request() {
        use crate::activity::NavRequest;

        // Confirm + Back in one batch → the search never leaves, and the release still goes out.
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.admit_navigator_intent(NavigatorIntent::PlanRoute(NavRequest::new((0, 0), (1, 1), "A")));
        app.admit_navigator_intent(NavigatorIntent::CancelPlan);
        assert_eq!(drain_nav(&mut app), None, "annihilated before any host saw it");
        assert!(drain_cancel(&mut app), "the cancel still latches (a stale cancel is a host no-op)");

        // Three gestures in one batch: Back on in-flight A's spinner, confirm B, Back on B's.
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.admit_navigator_intent(NavigatorIntent::PlanRoute(NavRequest::new((0, 0), (1, 1), "A")));
        assert!(drain_nav(&mut app).is_some(), "the host already holds plan A");
        app.admit_navigator_intent(NavigatorIntent::CancelPlan); // Back on A's spinner — nothing undrained to annihilate
        app.admit_navigator_intent(NavigatorIntent::PlanRoute(NavRequest::new((0, 0), (2, 2), "B"))); // confirm B
        app.admit_navigator_intent(NavigatorIntent::CancelPlan); // Back on B's spinner — annihilates the undrained B
        assert!(drain_cancel(&mut app), "one cancel: aborts the in-flight A");
        assert_eq!(drain_nav(&mut app), None, "B never runs");
    }

    // ==================== Auto-expiry sweep (epic #638, S3) — the safety invariants ====================

    use crate::retention::{Retention, RideRetention, RouteRetentionMeta, DAY_SECS};

    /// A known UTC set-point for the trusted-clock helper — mid-2026, offset 0.
    fn sweep_dt() -> DateTime {
        DateTime { year: 2026, month: 7, day: 14, hour: 12, minute: 0 }
    }

    /// A fresh app with a **trusted** GPS-stamped clock; returns it and the UTC `now` it reads.
    fn trusted_app() -> (App, u32) {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.stamp_clock(sweep_dt(), 0, None, ClockTrust::Gps);
        let now = app.wall_unix_now();
        (app, now)
    }

    /// Re-stamp the trusted clock `days` days later than `sweep_dt` (advancing `now`) and force the
    /// next sweep to run regardless of the hourly gate. Returns the new UTC `now`.
    fn advance_days(app: &mut App, days: u32) -> u32 {
        // The set-point is minute-resolution; advance via a fresh `stamp_clock` at a later date.
        let mut dt = sweep_dt();
        dt.day += days as u8; // stays within July for the small offsets these tests use
        app.stamp_clock(dt, 0, None, ClockTrust::Gps);
        app.force_retention_sweep();
        app.wall_unix_now()
    }

    fn synced_ride(name: &str, synced: bool, synced_at_utc: u32) -> crate::ride::RideSummary {
        crate::ride::RideSummary {
            name: heapless::String::try_from(name).unwrap(),
            start_time: 1_720_000_000,
            distance_m: 1_000,
            moving_time_s: 600,
            climb_m: 10,
            synced,
            synced_at_utc,
        }
    }

    /// What one pass asked the platform to do about retention.
    ///
    /// The old drain spelled these `DeleteRoute` / `DeleteRide` / `StampRouteUsed` /
    /// `StampRideSynced`. The pass emits `CatalogEffect::RemoveObject`, which names the *object* and
    /// not its namespace because the store removes by identity, and `RetentionEffect::Write*Metadata`
    /// for the sidecar writes.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum SweepOp {
        Remove(crate::CatalogObjectId),
        StampRoute(crate::CatalogObjectId),
        StampRide(crate::CatalogObjectId),
    }

    /// The retention executor these tests run: one pass at a time, serving whatever it asks for and
    /// reporting the ops it asked for. Its outcome slots are its own, exactly like a host's — an
    /// answer it deposits is read by the *next* pass, which is what makes "one operation in flight"
    /// observable.
    /// Every capability the test platform implements.
    const EVERY_CAPABILITY: crate::device_core::PlatformSupport = crate::device_core::PlatformSupport {
        detour: true,
        settings_persistence: true,
        dfu: true,
        weather: true,
        bonding: true,
        storage_space_report: true,
        retention_metadata: true,
    };

    /// A location source with nothing to say.
    struct NoFix;
    impl LocationSource for NoFix {
        fn poll(&mut self) -> Option<obc_ports::Fix> {
            None
        }
    }

    struct Sweeper {
        outcomes: crate::device_core::OutcomeSlots,
        ms: u32,
        /// What the store reports for a removal. `false` is a transient failure the domain retries.
        store_ok: bool,
    }

    impl Sweeper {
        fn new() -> Self {
            Sweeper { outcomes: crate::device_core::OutcomeSlots::new(), ms: 0, store_ok: true }
        }

        /// One pass: run it, record what it asked for, and answer it for the next one.
        fn pass(&mut self, app: &mut App) -> heapless::Vec<SweepOp, 8> {
            use crate::catalog_state::{CatalogEffect, CatalogError, CatalogOutcome};
            use crate::retention::{RetentionEffect, RetentionOutcome};
            let mut out: heapless::Vec<SweepOp, 8> = heapless::Vec::new();
            self.ms += 1;
            let ms = self.ms.max(app.ui.now_ms);
            let mut loc = NoFix;
            let mut facts = crate::device_core::ExternalFacts::NONE;
            let mut plan = app.run_pass(crate::device_core::PassInputs {
                now: crate::device_core::PassClock { ride: RideClock(ms), ui: obc_ports::InputClock(ms) },
                gestures: &[],
                sensors: Sensors::new(&mut loc),
                route: None,
                weather: None,
                support: EVERY_CAPABILITY,
                outcomes: &mut self.outcomes,
                facts: &mut facts,
                derived: crate::device_core::DerivedInputs::NONE,
                targets: crate::device_core::DerivedTargets::NONE,
            });
            if let Some(effect) = plan.effects.retention.take() {
                let token = effect.token();
                let outcome = match effect {
                    RetentionEffect::WriteRouteMetadata { id, .. } => {
                        let _ = out.push(SweepOp::StampRoute(id));
                        RetentionOutcome::RouteMetadataWritten { token, id }
                    }
                    RetentionEffect::WriteRideMetadata { id, .. } => {
                        let _ = out.push(SweepOp::StampRide(id));
                        RetentionOutcome::RideMetadataWritten { token, id }
                    }
                };
                let _ = self.outcomes.retention.try_put(outcome);
            }
            if let Some(effect) = plan.effects.catalog.take() {
                let token = effect.token();
                match effect {
                    CatalogEffect::RemoveObject { object, .. } => {
                        let _ = out.push(SweepOp::Remove(object));
                        let _ = self.outcomes.catalog.try_put(if self.store_ok {
                            CatalogOutcome::ObjectRemoved { token, object, existed: true }
                        } else {
                            CatalogOutcome::Failed { token, error: CatalogError::RemoveFailed }
                        });
                    }
                    // The re-read a completed removal orders (#1541). These tests re-feed the
                    // catalog themselves where they mean the store to have changed, so answering
                    // the operation is the whole of what this executor owes it.
                    CatalogEffect::ReadCatalog { .. } => {
                        let _ = self.outcomes.catalog.try_put(CatalogOutcome::CatalogRead { token });
                    }
                }
            }
            out
        }

        /// `n` passes, with everything they ask for served.
        fn rounds(&mut self, app: &mut App, n: usize) -> heapless::Vec<SweepOp, 8> {
            let mut out: heapless::Vec<SweepOp, 8> = heapless::Vec::new();
            for _ in 0..n {
                for op in self.pass(app) {
                    let _ = out.push(op);
                }
            }
            out
        }
    }

    /// Drive several retention-tick + pass rounds and collect every op produced. Multiple rounds are
    /// needed because a domain performs one bounded operation at a time and the once-per-activation
    /// stamp defers the batch sweep a tick; a round that produces nothing new ends the drive.
    fn sweep_and_drain(app: &mut App) -> heapless::Vec<SweepOp, 128> {
        let mut host = Sweeper::new();
        let mut out: heapless::Vec<SweepOp, 128> = heapless::Vec::new();
        for _ in 0..24 {
            let before = out.len();
            app.retention_tick();
            for _ in 0..4 {
                for op in host.pass(app) {
                    let _ = out.push(op);
                }
            }
            if out.len() == before {
                break; // a full round produced nothing new
            }
        }
        out
    }

    fn n_deletes(ops: &[SweepOp]) -> usize {
        ops.iter().filter(|op| matches!(op, SweepOp::Remove(_))).count()
    }

    /// Invariant 1: no trusted clock this boot → the sweep does nothing and stamps nothing, even
    /// with data that *looks* long expired.
    #[test]
    fn sweep_does_nothing_without_a_trusted_clock() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0)); // never stamped → Untrusted
        app.set_routes_with_meta(
            &[summary("Old")],
            &[10],
            &[RouteRetentionMeta::new(Retention::Day1, 1)], // "used" at unix 1 → ancient
        );
        app.set_rides(&[synced_ride("R", true, 1)], &[7]);
        let cmds = sweep_and_drain(&mut app);
        assert!(cmds.is_empty(), "untrusted clock → no deletes, no stamps: {cmds:?}");
    }

    /// Invariant 6 + the delete happy-path: a trusted sweep deletes an expired route, keeps a fresh
    /// one, and never touches a `Never` route.
    #[test]
    fn sweep_deletes_expired_keeps_fresh_and_never() {
        let (mut app, now) = trusted_app();
        app.set_routes_with_meta(
            &[summary("Expired"), summary("Fresh"), summary("Forever")],
            &[10, 11, 12],
            &[
                RouteRetentionMeta::new(Retention::Day1, now - 3 * DAY_SECS),
                RouteRetentionMeta::new(Retention::Week1, now - DAY_SECS),
                RouteRetentionMeta::new(Retention::Never, 1),
            ],
        );
        let cmds = sweep_and_drain(&mut app);
        assert!(cmds.contains(&SweepOp::Remove(10)), "expired route deleted");
        assert!(!cmds.iter().any(|c| matches!(c, SweepOp::Remove(11 | 12))), "fresh + Never kept");
    }

    /// Invariant 2: a retention-set route with an **unknown** `last_used` is stamped (the clock
    /// starts) — never deleted on sight — and only deletes after the full period from that stamp.
    #[test]
    fn sweep_starts_the_clock_then_deletes_after_the_period() {
        let (mut app, _now) = trusted_app();
        app.set_routes_with_meta(&[summary("New")], &[10], &[RouteRetentionMeta::new(Retention::Day1, 0)]);
        let cmds = sweep_and_drain(&mut app);
        assert!(cmds.iter().any(|c| matches!(c, SweepOp::StampRoute(10))), "clock started");
        assert_eq!(n_deletes(&cmds), 0, "unknown last_used is never deleted on sight");
        // The stamp's optimistic mirror set last_used = now; a forced re-sweep at the same instant
        // finds it freshly stamped and well within the 1-day window — nothing deletes.
        app.force_retention_sweep();
        assert_eq!(n_deletes(&sweep_and_drain(&mut app)), 0, "freshly stamped — not expired");
        // Days past the 1-day window it deletes.
        advance_days(&mut app, 5);
        assert!(sweep_and_drain(&mut app).contains(&SweepOp::Remove(10)), "deletes after the period");
    }

    /// Invariant 3: the active navigation route is never deleted — it re-stamps when it would expire.
    #[test]
    fn sweep_never_deletes_the_active_route() {
        let (mut app, now) = trusted_app();
        app.set_routes_with_meta(
            &[summary("Active"), summary("Idle")],
            &[10, 11],
            &[
                RouteRetentionMeta::new(Retention::Day1, now - 5 * DAY_SECS), // active + long expired
                RouteRetentionMeta::new(Retention::Day1, now - 5 * DAY_SECS), // inactive + long expired
            ],
        );
        app.activate_route(0); // route 10 is the active nav route
        let cmds = sweep_and_drain(&mut app);
        assert!(cmds.iter().any(|c| matches!(c, SweepOp::StampRoute(10))), "active re-stamped");
        assert!(!cmds.contains(&SweepOp::Remove(10)), "the active route is never deleted");
        assert!(cmds.contains(&SweepOp::Remove(11)), "the idle expired route is deleted");
    }

    /// The route-upload `last_used` stamp (epic #638 S4): a committed upload under a **trusted** clock
    /// enqueues a `StampRouteUsed` for the route — anchoring its expiry clock at upload time — while an
    /// upload under an **untrusted** clock stamps nothing (the sweep starts the clock later, invariant
    /// 2). A fresh route is `Never` at upload (the app sets real retention via a later
    /// `setRouteRetention`), yet the upload still anchors `last_used` so the eventual expiry counts
    /// from upload time.
    #[test]
    fn route_upload_stamps_last_used_only_when_trusted() {
        fn upload_and_drain(app: &mut App) -> heapless::Vec<SweepOp, 128> {
            app.on_route_uploaded(10, false, None);
            sweep_and_drain(app)
        }

        // Trusted: an upload commit stamps the route used (anchoring the expiry clock at upload time).
        let (mut app, _now) = trusted_app();
        app.set_routes_with_meta(&[summary("Fresh")], &[10], &[RouteRetentionMeta::new(Retention::Never, 0)]);
        let cmds = upload_and_drain(&mut app);
        assert!(
            cmds.iter().any(|c| matches!(c, SweepOp::StampRoute(10))),
            "a trusted upload stamps last_used: {cmds:?}"
        );

        // Untrusted (never stamped this boot): the same upload stamps nothing — the safe fallback.
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.set_routes_with_meta(&[summary("Fresh")], &[10], &[RouteRetentionMeta::new(Retention::Never, 0)]);
        let cmds = upload_and_drain(&mut app);
        assert!(
            !cmds.iter().any(|c| matches!(c, SweepOp::StampRoute(_))),
            "an untrusted upload stamps nothing: {cmds:?}"
        );
    }

    /// #1548: the upload stamp does **not** take its sibling's expiring-route filter. Every fresh
    /// route is `Never` at upload — the app sets the level in a separate command that never touches
    /// `last_used` — so filtering `Never` here would slip the anchor to the next hourly sweep, which
    /// is the imprecision this stamp exists to remove.
    #[test]
    fn a_never_route_uploaded_and_levelled_later_anchors_at_upload() {
        let (mut app, now) = trusted_app();
        app.set_routes_with_meta(&[summary("Fresh")], &[10], &[RouteRetentionMeta::new(Retention::Never, 0)]);
        app.on_route_uploaded(10, false, None);
        sweep_and_drain(&mut app);
        let anchored = app.route_metas()[0].last_used_utc;
        assert_eq!(anchored, now, "the upload anchored `last_used`, not the next sweep");

        // The phone's `setRouteRetention` sets the level and leaves `last_used` alone.
        app.set_route_meta(&[RouteRetentionMeta::new(Retention::Day1, anchored)]);
        assert_eq!(app.route_metas()[0].expires_at(), Some(now + DAY_SECS), "the countdown runs from the upload");
    }

    /// Invariant 4: no sweep (no deletions) while a ride is recording — even with an expired route.
    #[test]
    fn sweep_suppressed_while_recording() {
        let (mut app, now) = trusted_app();
        app.set_routes_with_meta(
            &[summary("Expired")],
            &[10],
            &[RouteRetentionMeta::new(Retention::Day1, now - 3 * DAY_SECS)],
        );
        app.test_start_ride(); // recording in progress
        let cmds = sweep_and_drain(&mut app);
        assert_eq!(n_deletes(&cmds), 0, "recording suppresses the sweep — nothing deleted");
    }

    /// A ride acked synced under a **trusted clock while recording** gets its `synced_at` stamped
    /// **at ack-time** (its countdown starts) — the eager stamp is *not* deferred to the
    /// recording-gated delete sweep. A metadata stamp is safe mid-ride; only deletions wait for
    /// recording to end (invariant 4). (Regression guard for the S3 review fix.)
    #[test]
    fn ride_synced_at_stamped_eagerly_even_while_recording() {
        let (mut app, _now) = trusted_app();
        app.test_start_ride(); // recording a multi-day tour
                               // The phone acks a ride synced (synced_at not yet set) mid-recording.
        app.set_rides(&[synced_ride("Acked", true, 0)], &[7]);
        let cmds = sweep_and_drain(&mut app);
        assert!(
            cmds.iter().any(|c| matches!(c, SweepOp::StampRide(7))),
            "the countdown starts at ack-time, not deferred to recording-end: {cmds:?}"
        );
        assert_eq!(n_deletes(&cmds), 0, "but nothing is deleted while recording");
        // The stamp mirrored synced_at = now, so it isn't re-enqueued on the next tick.
        app.force_retention_sweep();
        let again = sweep_and_drain(&mut app);
        assert!(
            !again.iter().any(|c| matches!(c, SweepOp::StampRide(7))),
            "a stamped ride is not re-stamped: {again:?}"
        );
    }

    /// Invariant 5: rides — unsynced is untouched at any age; synced + aged deletes; synced +
    /// `synced_at == 0` (legacy) is stamped then later deletes; `ride_retention = Never` deletes
    /// nothing.
    #[test]
    fn sweep_ride_rules_end_to_end() {
        let (mut app, now) = trusted_app();
        app.set_settings(Settings { ride_retention: RideRetention::Week1, ..Settings::default() });
        // Re-stamp trust (set_settings re-stamped the wall clock from the persisted set-point).
        app.stamp_clock(sweep_dt(), 0, None, ClockTrust::Gps);
        let now = app.wall_unix_now().max(now);
        app.set_rides(
            &[
                synced_ride("Aged", true, now - 8 * DAY_SECS), // synced 8d ago → delete (>7)
                synced_ride("Recent", true, now - DAY_SECS),   // synced 1d ago → keep
                synced_ride("Legacy", true, 0),                // synced, no stamp → stamp
                synced_ride("Unsynced", false, 0),             // unsynced → never touched
            ],
            &[1, 2, 3, 4],
        );
        let cmds = sweep_and_drain(&mut app);
        assert!(cmds.contains(&SweepOp::Remove(1)), "aged synced ride deleted");
        assert!(!cmds.contains(&SweepOp::Remove(2)), "recent synced ride kept");
        assert!(cmds.iter().any(|c| matches!(c, SweepOp::StampRide(3))), "legacy ride stamped");
        assert!(!cmds.iter().any(|c| matches!(c, SweepOp::Remove(3))), "legacy ride not deleted on sight");
        assert!(
            !cmds.iter().any(|c| matches!(c, SweepOp::Remove(4) | SweepOp::StampRide(4))),
            "the unsynced ride is never touched"
        );
    }

    /// `ride_retention = Never` deletes no ride, however long ago it synced.
    #[test]
    fn sweep_ride_retention_never_deletes_nothing() {
        let (mut app, _now) = trusted_app();
        app.set_settings(Settings { ride_retention: RideRetention::Never, ..Settings::default() });
        app.stamp_clock(sweep_dt(), 0, None, ClockTrust::Gps);
        app.set_rides(&[synced_ride("Aged", true, 1)], &[1]); // synced at unix 1 → ancient
        assert_eq!(n_deletes(&sweep_and_drain(&mut app)), 0, "ride_retention Never → nothing");
    }

    /// Exact boundary: `now == expires_at` deletes (the `>=` in the policy).
    #[test]
    fn sweep_deletes_on_the_exact_boundary() {
        let (mut app, now) = trusted_app();
        // last_used = now - 1 day, retention 1 day → expires_at == now exactly.
        app.set_routes_with_meta(
            &[summary("Boundary")],
            &[10],
            &[RouteRetentionMeta::new(Retention::Day1, now - DAY_SECS)],
        );
        assert!(sweep_and_drain(&mut app).contains(&SweepOp::Remove(10)), "now == expires_at deletes");
    }

    /// Remap coherence: a route delete mid-session keeps each surviving route's retention meta
    /// aligned with its id across the rescan.
    #[test]
    fn route_meta_stays_aligned_across_a_rescan() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.set_routes_with_meta(
            &[summary("A"), summary("B"), summary("C")],
            &[10, 11, 12],
            &[
                RouteRetentionMeta::new(Retention::Day1, 100),
                RouteRetentionMeta::new(Retention::Week1, 200),
                RouteRetentionMeta::new(Retention::Month1, 300),
            ],
        );
        // A rescan drops the middle route (id 11) — B is gone, A and C survive in a new order.
        app.set_routes_with_ids(&[summary("C"), summary("A")], &[12, 10]);
        assert_eq!(app.route_ids(), &[12, 10]);
        let metas = app.route_metas();
        assert_eq!(metas[0], RouteRetentionMeta::new(Retention::Month1, 300), "C's meta followed its id");
        assert_eq!(metas[1], RouteRetentionMeta::new(Retention::Day1, 100), "A's meta followed its id");
    }

    // ==================== finding #876: just-in-time execution guards ====================
    //
    // The tests above collect *and* dispatch in one `sweep_and_drain`, so a decision never goes
    // stale between the two. These drive the race the issue is about: fill the candidate queue with
    // one `retention_tick`, mutate live state, and prove the **drain** re-derives the decision.

    use crate::retention::{RideRetentionRecord, SweepKind, RETENTION_DELETE_BACKOFF_MS};

    /// One executor round, collecting the ops it produced. A domain performs one bounded
    /// operation at a time, so a route delete and a ride delete are two catalog operations and
    /// leave on consecutive passes.
    /// Three passes, because that is one whole catalog operation from this executor's side: the
    /// effect goes out, its answer comes back, and the re-read the answer orders (#1541) is served
    /// too. Each call builds a fresh [`Sweeper`], so an answer left unconsumed at the last pass
    /// would be dropped with it and the domain would stay in flight for the rest of the test.
    fn drain_once(app: &mut App) -> heapless::Vec<SweepOp, 8> {
        Sweeper::new().rounds(app, 3)
    }

    /// The same round against a store that **refuses** every removal — the one case the backstop
    /// still covers, now that a completed removal is retired by the catalog's verdict.
    fn drain_once_refusing(app: &mut App) -> heapless::Vec<SweepOp, 8> {
        Sweeper { store_ok: false, ..Sweeper::new() }.rounds(app, 3)
    }

    fn expired(now: u32) -> RouteRetentionMeta {
        RouteRetentionMeta::new(Retention::Day1, now - 3 * DAY_SECS)
    }

    /// Finding #876-1: a route **activated after the sweep discovered it** as a delete candidate but
    /// **before the delete drains** is never deleted — the live drain recheck converts it to a
    /// re-stamp, and the still-idle expired route deletes as normal.
    #[test]
    fn activation_after_discovery_cancels_the_queued_delete() {
        let (mut app, now) = trusted_app();
        app.set_routes_with_meta(&[summary("A"), summary("B")], &[10, 11], &[expired(now), expired(now)]);
        app.retention_tick(); // the sweep queues DeleteRoute(10) + DeleteRoute(11)
        assert!(app.retention.has(SweepKind::DeleteRoute), "both routes are delete candidates");
        // The rider opens route 10 and starts navigating it before that item drains.
        app.activate_route(0);
        let cmds = drain_once(&mut app);
        assert!(!cmds.contains(&SweepOp::Remove(10)), "the activated route is never deleted");
        assert!(
            cmds.iter().any(|c| matches!(c, SweepOp::StampRoute(10))),
            "the activated route is re-stamped instead: {cmds:?}"
        );
        assert!(cmds.contains(&SweepOp::Remove(11)), "the still-idle expired route deletes");
    }

    /// Finding #876-1 (invariant 4): deletes discovered while idle and **then** interrupted by a
    /// recording are deferred — not dropped — and dispatch once recording ends.
    #[test]
    fn recording_after_discovery_defers_deletes_without_losing_them() {
        let (mut app, now) = trusted_app();
        app.set_routes_with_meta(&[summary("R")], &[10], &[expired(now)]);
        app.set_ride_retention_inventory(&[RideRetentionRecord {
            id: 7,
            synced: true,
            synced_at_utc: now - 8 * DAY_SECS,
        }]);
        app.retention_tick(); // discovers DeleteRoute(10) + DeleteRide(7) while idle
        assert!(app.retention.has(SweepKind::DeleteRoute) && app.retention.has(SweepKind::DeleteRide));
        // Recording begins *after* discovery, on a later frame.
        app.test_start_ride();
        let while_recording = drain_once(&mut app);
        assert_eq!(n_deletes(&while_recording), 0, "no auto-delete dispatches while recording");
        assert!(
            app.retention.has(SweepKind::DeleteRoute) && app.retention.has(SweepKind::DeleteRide),
            "the candidates are retained, not dropped"
        );
        // Recording ends → the same candidates dispatch (route + ride are separate classes → one pass).
        app.test_end_ride();
        let after = drain_once(&mut app);
        assert!(after.contains(&SweepOp::Remove(10)), "the route delete dispatches after recording");
        assert!(after.contains(&SweepOp::Remove(7)), "the ride delete dispatches after recording");
    }

    /// Finding #876-1: retention/metadata changed **between discovery and dispatch** — the live state
    /// wins. A route lengthened to `Never` after the sweep queued its delete is not deleted.
    #[test]
    fn metadata_change_between_discovery_and_dispatch_wins() {
        let (mut app, now) = trusted_app();
        app.set_routes_with_meta(&[summary("R")], &[10], &[expired(now)]);
        app.retention_tick(); // queues DeleteRoute(10)
        assert!(app.retention.has(SweepKind::DeleteRoute));
        // The phone sets this route to Never (or re-stamps it) before the delete drains.
        app.set_route_meta(&[RouteRetentionMeta::new(Retention::Never, now - 3 * DAY_SECS)]);
        let cmds = drain_once(&mut app);
        assert_eq!(n_deletes(&cmds), 0, "the live Never wins — the stale delete candidate is cancelled");
        assert!(!app.retention.has(SweepKind::DeleteRoute), "the cancelled candidate is retired");
    }

    /// Finding #876-3: multiple expired objects are drained **one in flight at a time**, and every id
    /// is executed exactly once (or resolved already-absent) — none is overwritten or dropped, the
    /// exact failure the coalescing delete `Signal` had. The host "applies" each delete by rescanning
    /// the store without the id (as the real store-changed edge does).
    #[test]
    fn batched_deletes_all_execute_exactly_once() {
        let (mut app, now) = trusted_app();
        let mut live: heapless::Vec<crate::CatalogObjectId, 4> = heapless::Vec::from_slice(&[10, 11, 12]).unwrap();
        let rescan = |app: &mut App, live: &[crate::CatalogObjectId]| {
            let sums: heapless::Vec<RouteSummary, 4> = live.iter().map(|_| summary("x")).collect();
            let metas: heapless::Vec<RouteRetentionMeta, 4> = live.iter().map(|_| expired(now)).collect();
            app.set_routes_with_meta(&sums, live, &metas);
        };
        rescan(&mut app, &live);
        app.retention_tick(); // three delete candidates queued at once

        let mut deleted: heapless::Vec<crate::CatalogObjectId, 8> = heapless::Vec::new();
        for _ in 0..12 {
            for c in &drain_once(&mut app) {
                if let SweepOp::Remove(id) = c {
                    let _ = deleted.push(*id);
                    // Storage succeeds: the id leaves the catalog on the next rescan.
                    if let Some(p) = live.iter().position(|x| x == id) {
                        live.remove(p);
                    }
                    rescan(&mut app, &live);
                }
            }
            if !app.retention.has(SweepKind::DeleteRoute) {
                break;
            }
        }
        assert_eq!(deleted.len(), 3, "every expired route was deleted: {deleted:?}");
        for id in [10u64, 11, 12] {
            assert_eq!(deleted.iter().filter(|&&x| x == id).count(), 1, "id {id} executed exactly once");
        }
    }

    /// Finding #876-3, now the backstop's own case (#1548): a **refused** removal keeps its
    /// candidate and retries it — no second hourly sweep is needed — paced by the bounded window so
    /// a dead card is not hammered every frame.
    #[test]
    fn a_refused_removal_keeps_its_candidate_and_retries_after_the_backoff() {
        let (mut app, now) = trusted_app();
        app.set_routes_with_meta(&[summary("A")], &[10], &[expired(now)]);
        app.retention_tick();
        assert!(drain_once_refusing(&mut app).contains(&SweepOp::Remove(10)), "first dispatch");
        // The store refused: route 10 is still there, and nothing retired the candidate.
        assert!(
            !drain_once_refusing(&mut app).contains(&SweepOp::Remove(10)),
            "the backstop paces the retry — no per-frame hammering"
        );
        // Past the window, the *same* candidate re-dispatches — no new sweep ran in between.
        app.ui.now_ms += RETENTION_DELETE_BACKOFF_MS + 1;
        assert!(
            drain_once_refusing(&mut app).contains(&SweepOp::Remove(10)),
            "the retained candidate retries itself, without another hourly discovery"
        );
    }

    /// One in-flight slot, not one per class (#1548): the backstop belongs to the **store**, so a
    /// ride removal does not walk into a card that refused a route removal a frame ago. It follows
    /// once the window has passed, or — on the ordinary path — in the pass the route's verdict lands.
    #[test]
    fn a_refused_removal_paces_the_next_delete_of_either_kind() {
        let (mut app, now) = trusted_app();
        app.set_routes_with_meta(&[summary("R")], &[10], &[expired(now)]);
        app.set_ride_retention_inventory(&[RideRetentionRecord {
            id: 7,
            synced: true,
            synced_at_utc: now - 8 * DAY_SECS,
        }]);
        app.retention_tick();
        let first = drain_once_refusing(&mut app);
        assert!(first.contains(&SweepOp::Remove(10)), "the route removal goes out first: {first:?}");
        assert_eq!(n_deletes(&first), 1, "the card just refused — the ride does not walk into it: {first:?}");

        let blocked = drain_once_refusing(&mut app);
        assert_eq!(n_deletes(&blocked), 0, "and it still waits inside the window: {blocked:?}");
        app.ui.now_ms += RETENTION_DELETE_BACKOFF_MS + 1;
        assert!(drain_once_refusing(&mut app).contains(&SweepOp::Remove(10)), "past the window the head retries");
    }

    /// The class order is the domain's, not the stage's (#1548): a route expiry is offered before a
    /// ride expiry, which is the order the sweep discovers them in.
    #[test]
    fn an_expiry_offers_a_route_before_a_ride() {
        let (mut app, now) = trusted_app();
        app.set_routes_with_meta(&[summary("R")], &[10], &[expired(now)]);
        app.set_ride_retention_inventory(&[RideRetentionRecord {
            id: 7,
            synced: true,
            synced_at_utc: now - 8 * DAY_SECS,
        }]);
        app.retention_tick();
        let mut host = Sweeper::new();
        assert_eq!(host.pass(&mut app).first(), Some(&SweepOp::Remove(10)), "the route class goes first");
        assert!(host.rounds(&mut app, 2).contains(&SweepOp::Remove(7)), "and the ride follows it");
    }

    /// #1548: a **completed** removal retires the expiry candidate in the pass its answer lands —
    /// while the resident catalogs are still the pre-removal picture, because the re-read the
    /// removal ordered has not run yet.
    #[test]
    fn a_completed_removal_retires_its_expiry_candidate_in_the_same_pass() {
        let (mut app, now) = trusted_app();
        app.set_routes_with_meta(&[summary("A")], &[10], &[expired(now)]);
        app.retention_tick();
        let mut host = Sweeper::new();
        assert!(host.pass(&mut app).contains(&SweepOp::Remove(10)), "the expiry dispatches");

        host.pass(&mut app); // the answer lands at stage 1 of this pass
        assert_eq!(app.route_ids(), &[10], "the catalogs are still behind the store");
        assert!(!app.retention.has(SweepKind::DeleteRoute), "and the candidate is already retired");
    }

    /// #1548: every producer of a deletion converges on the same verdict. A route the **rider**
    /// deletes retires the expiry candidate the sweep had for it, so the object is removed once.
    #[test]
    fn a_riders_delete_retires_the_expiry_candidate_for_the_same_object() {
        let (mut app, now) = trusted_app();
        app.set_routes_with_meta(&[summary("A"), summary("B")], &[10, 11], &[expired(now), expired(now)]);
        app.retention_tick(); // both routes are expiry candidates, and 10 is the head
        app.activity.request_route_delete(1); // the rider deletes 11 by hand, so the sweep never did

        let mut ops: heapless::Vec<SweepOp, 32> = heapless::Vec::new();
        for _ in 0..4 {
            for op in drain_once(&mut app) {
                let _ = ops.push(op);
            }
        }
        for id in [10u64, 11] {
            let n = ops.iter().filter(|op| **op == SweepOp::Remove(id)).count();
            assert_eq!(n, 1, "{id} removed once, whoever ordered it: {ops:?}");
        }
        assert!(!app.retention.has(SweepKind::DeleteRoute), "both candidates were retired by their verdicts");
    }

    /// #1548: the same race one pass tighter. The rider deletes the route the sweep's **head**
    /// candidate names, so both reach the catalog in one pass: the rider's is admitted and the
    /// expiry is refused and parked. When the verdict lands, the parked intent is a copy of a
    /// candidate that is already retired — admitting it would remove an object that has gone.
    #[test]
    fn a_riders_delete_of_the_head_candidate_orders_one_removal() {
        let (mut app, now) = trusted_app();
        app.set_routes_with_meta(&[summary("A")], &[10], &[expired(now)]);
        app.retention_tick();
        app.activity.request_route_delete(0);

        let mut ops: heapless::Vec<SweepOp, 32> = heapless::Vec::new();
        for _ in 0..3 {
            for op in drain_once(&mut app) {
                let _ = ops.push(op);
            }
        }
        assert_eq!(n_deletes(&ops), 1, "the rider's removal is the only one: {ops:?}");
    }

    /// #1548 finding 2: the store **answered `ObjectRemoved`**, and the re-read that answer ordered
    /// has not re-fed the catalogs yet — the object is still a resident row. That is the ordinary
    /// board cadence, not a fault: the pass clock is real monotonic millis and the device sleeps
    /// between wakes, so more than [`RETENTION_DELETE_BACKOFF_MS`] routinely elapses between the
    /// answer and the read landing. A second removal for an object the store has already removed is
    /// a second `ObjectRemoved`, a second armed re-read and a second wake, and it breaks #1541's
    /// "one read per delete".
    #[test]
    fn an_expiry_answered_removed_is_not_dispatched_twice_when_the_re_read_is_slow() {
        let (mut app, now) = trusted_app();
        app.set_routes_with_meta(&[summary("A")], &[10], &[expired(now)]);
        app.retention_tick();
        assert!(drain_once(&mut app).contains(&SweepOp::Remove(10)), "the expiry dispatches once");

        // The removal was answered `ObjectRemoved`. The catalogs are still the pre-removal picture
        // until the owed read lands, and the device slept past the backoff in the meantime.
        app.ui.now_ms += RETENTION_DELETE_BACKOFF_MS + 1;
        let later = drain_once(&mut app);
        assert!(
            !later.contains(&SweepOp::Remove(10)),
            "the verdict retired the candidate — a slow re-read must not order a second removal: {later:?}"
        );
    }

    /// Review fix (#886): cancelling a queued-but-never-dispatched candidate must NOT re-open the
    /// dispatch window while a *different* id's removal is outstanding. Interleaving:
    /// `DeleteRoute(10)` is dispatched and refused, `DeleteRoute(11)` is queued behind it; the
    /// rider activates route 11 → `note_active_route` cancels 11's candidate. The same-pass drain
    /// must not re-emit `DeleteRoute(10)` mid-flight, and 10 must stay retained for its own retry.
    #[test]
    fn cancel_of_a_queued_candidate_does_not_reopen_the_inflight_window() {
        let (mut app, now) = trusted_app();
        app.set_routes_with_meta(&[summary("X"), summary("A")], &[10, 11], &[expired(now), expired(now)]);
        app.retention_tick(); // queues DeleteRoute(10) + DeleteRoute(11)
                              // The store refuses, so 10 stays in flight instead of being retired by its own verdict.
        let first = drain_once_refusing(&mut app);
        assert!(first.contains(&SweepOp::Remove(10)), "10 dispatches first and is in flight");
        assert!(!first.contains(&SweepOp::Remove(11)), "one delete in flight at a time");

        // The rider activates route 11 (catalog index 1) — the next tick's `note_active_route`
        // cancels 11's queued (never-dispatched) delete candidate.
        app.activate_route(1);
        app.retention_tick();
        assert!(!app.retention.has(SweepKind::StampRide), "sanity: only route work is queued");
        let cmds = drain_once_refusing(&mut app);
        assert!(
            !cmds.iter().any(|c| matches!(c, SweepOp::Remove(_))),
            "cancelling 11 must not re-emit the in-flight 10 mid-backoff: {cmds:?}"
        );
        assert_eq!(app.retention.peek(SweepKind::DeleteRoute), Some(10), "10 stays retained for its own retry");

        // 10's own backoff still governs its retry: past the window, 10 (and only 10) re-dispatches.
        app.ui.now_ms += RETENTION_DELETE_BACKOFF_MS + 1;
        let retry = drain_once(&mut app);
        assert!(retry.contains(&SweepOp::Remove(10)), "10 retries after its backoff");
        assert!(!retry.contains(&SweepOp::Remove(11)), "the activated 11 is never deleted");
    }

    /// Review fix (#886): the drain-time trust guard. Delete candidates that are already queued
    /// (collected under a trusted clock) are **retained and nothing dispatches** when the clock is
    /// not trusted at drain time — invariant 1 holds at execution time, not only at collection time.
    /// (Production trust never reverts within a boot; the guard is belt-and-braces, exercised here
    /// by seeding candidates directly into an untrusted app.)
    #[test]
    fn untrusted_clock_at_drain_time_defers_queued_deletes() {
        use crate::retention::SweepAction;
        let mut app = App::new_idle(AppState::new(0, 0, 1.0)); // never stamped → Untrusted
        app.set_routes_with_meta(&[summary("Old")], &[10], &[RouteRetentionMeta::new(Retention::Day1, 1)]);
        app.set_ride_retention_inventory(&[RideRetentionRecord { id: 7, synced: true, synced_at_utc: 1 }]);
        // Candidates as an earlier trusted sweep would have queued them.
        app.retention.test_push(SweepAction::DeleteRoute(10));
        app.retention.test_push(SweepAction::DeleteRide(7));

        let cmds = drain_once(&mut app);
        assert_eq!(n_deletes(&cmds), 0, "no trusted clock at drain time → nothing dispatches: {cmds:?}");
        assert!(
            app.retention.has(SweepKind::DeleteRoute) && app.retention.has(SweepKind::DeleteRide),
            "the candidates are retained (deferred), not dropped"
        );

        // Trust arrives → the same candidates dispatch (their live recheck still holds them due).
        app.stamp_clock(sweep_dt(), 0, None, ClockTrust::Gps);
        let after = drain_once(&mut app);
        assert!(after.contains(&SweepOp::Remove(10)), "route delete dispatches once trusted");
        assert!(after.contains(&SweepOp::Remove(7)), "ride delete dispatches once trusted");
    }

    // ==================== WX12 (#1197): travel direction, ride projection, alert engine ====================

    /// One tick with `fix` against the Grimsel fixture route (map-plane clock = `now_ms`).
    fn tick_route_fix(app: &mut App, route: &RouteReader, fix: Fix, now_ms: u32) {
        app.ui.now_ms = now_ms;
        let mut loc = OneFix(Some(fix));
        app.tick(RideClock(now_ms), Sensors::new(&mut loc), Some(route));
    }

    /// The WX12 travel-direction chain end to end: on-route fixes yield the route's general
    /// heading ahead of the matched progress (held while stopped — a rest stop still knows the
    /// ride's direction), and everything else is neutral, never a fabricated head/tail. Off-route
    /// the GPS course does **not** stand in, however fast the rider is moving (owner tuning round):
    /// the momentary heading is arbitrary the moment they stop or turn the bars.
    #[test]
    fn travel_direction_follows_the_route_heading_else_neutral() {
        let idx = grimsel_index();
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.activity.active_route = Some(0);
        assert_eq!(app.travel_deg(), None, "no fix yet → neutral");

        // A moving on-route fix: the travel direction is the route heading, not the (deliberately
        // contradictory) GPS course.
        let p = route.position_at(1_000).unwrap();
        let on_route = Fix { lat: p.lat, lon: p.lon, course: Some(275.0), speed_mps: Some(4.0) };
        tick_route_fix(&mut app, &route, on_route, 1_000);
        assert!(!app.activity.off_route);
        let heading = crate::weather::route_heading_deg(&route, app.activity.progress_m).unwrap();
        let travel = app.travel_deg().expect("on-route: the route heading");
        assert!((travel - heading).abs() < 0.01, "travel {travel} == heading {heading}");

        // Stopped on the route (no course, no speed): the heading is *held* — the wind question
        // at a rest stop is about the ride ahead.
        tick_route_fix(&mut app, &route, Fix { lat: p.lat, lon: p.lon, course: None, speed_mps: None }, 2_000);
        assert_eq!(app.travel_deg(), Some(travel), "held while stopped");

        // Far off the route, moving with a course: neutral. The GPS course used to stand in here,
        // and it is exactly the claim a rider standing at a junction can't trust.
        let far = Fix { lat: p.lat + 200_000, lon: p.lon, course: Some(123.0), speed_mps: Some(5.0) };
        tick_route_fix(&mut app, &route, far, 3_000);
        assert!(app.activity.off_route);
        assert_eq!(app.travel_deg(), None, "off-route → neutral, course or no course");

        // Off the route and stationary: still neutral.
        let parked = Fix { lat: p.lat + 200_000, lon: p.lon, course: None, speed_mps: Some(0.0) };
        tick_route_fix(&mut app, &route, parked, 4_000);
        assert_eq!(app.travel_deg(), None, "off-route stopped → neutral");

        // Unloading the route drops the heading with the rest of the derived state.
        tick_route_fix(&mut app, &route, on_route, 5_000);
        assert!(app.travel_deg().is_some());
        app.activity.active_route = None;
        app.drop_route_derived_state();
        assert_eq!(app.travel_deg(), None, "route unload → neutral until re-derived");
    }

    /// The other half of the same off-route switch (review F1): when `travel_deg` stops trusting
    /// the route tangent, `ride_projection` must stop trusting the route *position* too. A rider
    /// 20 km off the line still has a `progress_m` — the last match — and projecting the two-hour
    /// decision along it would answer for a route they aren't on. `None` there falls the host back
    /// to WX11's rider-position sampling, and re-matching restores the projection.
    #[test]
    fn ride_projection_refuses_to_project_along_a_route_the_rider_left() {
        let idx = grimsel_index();
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.activity.active_route = Some(0);

        // On the route: a projection, anchored at the matched progress.
        let p = route.position_at(1_000).unwrap();
        let on_route = Fix { lat: p.lat, lon: p.lon, course: Some(275.0), speed_mps: Some(4.0) };
        tick_route_fix(&mut app, &route, on_route, 1_000);
        assert!(!app.activity.off_route);
        let progress = app.activity.progress_m;
        assert_eq!(app.ride_projection().map(|p| p.progress_m), Some(progress));

        // 20 km off it: the matcher keeps the stale progress, the projection refuses to use it.
        let far = Fix { lat: p.lat + 200_000, lon: p.lon, course: Some(123.0), speed_mps: Some(5.0) };
        tick_route_fix(&mut app, &route, far, 2_000);
        assert!(app.activity.off_route);
        assert_eq!(app.travel_deg(), None, "the wind arrows already went neutral off the line…");
        assert_eq!(app.ride_projection(), None, "…and the ride decision must switch off the route too");

        // Back on the line: the projection returns.
        tick_route_fix(&mut app, &route, on_route, 3_000);
        assert!(!app.activity.off_route);
        assert!(app.ride_projection().is_some(), "re-matching restores the projection");
    }

    /// `ride_projection` bundles the matched progress with the recent moving **median** pace —
    /// capped against GPS teleports, with the documented touring fallback while stopped — and
    /// exists only once the matcher has locked onto an active route.
    #[test]
    fn ride_projection_pace_median_cap_and_fallback() {
        let idx = grimsel_index();
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);
        let mut app = App::new(AppState::new(0, 0, 1.0));
        assert_eq!(app.ride_projection(), None, "no active route → no projection");
        app.activity.active_route = Some(0);
        assert_eq!(app.ride_projection(), None, "route not matched yet → no projection");

        // A stationary lock: no moving sample yet → the touring fallback pace.
        let p = route.position_at(500).unwrap();
        tick_route_fix(&mut app, &route, Fix { lat: p.lat, lon: p.lon, course: None, speed_mps: Some(0.0) }, 1_000);
        let proj = app.ride_projection().expect("matched route → projection");
        assert_eq!(proj.speed_cms, crate::weather::TOURING_FALLBACK_CMS, "stopped → touring fallback");
        assert_eq!(proj.progress_m, app.activity.progress_m);

        // Moving samples: the median of 3/5/60 m/s — the 60 m/s teleport is capped to 15, and the
        // median (5 m/s) is immune to it anyway.
        for (i, mps) in [3.0f32, 5.0, 60.0].into_iter().enumerate() {
            let q = route.position_at(500 + i as u32 * 10).unwrap();
            tick_route_fix(
                &mut app,
                &route,
                Fix { lat: q.lat, lon: q.lon, course: None, speed_mps: Some(mps) },
                2_000 + i as u32 * 1_000,
            );
        }
        assert_eq!(app.ride_projection().unwrap().speed_cms, 500, "median of {{300, 500, 1500(capped)}}");
    }

    /// One rendered Hourly frame: `travel` is the WX12 travel direction the wind arrows classify
    /// against, `precip_tenth_mm` the amount every row carries (the rows are otherwise the
    /// [`alert_snap`] hourlies — wind from 200° at 4 m/s, condition RAIN).
    fn hourly_frame(travel: Option<f32>, precip_tenth_mm: u16) -> crate::harness::support::Buf {
        use crate::harness::support::{build_min_obcm, Buf};
        use embedded_graphics::pixelcolor::Rgb888;
        use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader};

        let mut app = App::new(AppState::new(0, 0, 1.0));
        let mut snap = alert_snap(&app, &[0; 9]);
        for record in snap.hourly.iter_mut() {
            record.precipitation_tenth_mm = precip_tenth_mm;
        }
        app.ride.travel_deg = travel;
        let _ = app.ui.stack.push(Screen::WeatherHourly(crate::screen::WeatherHourlyScreen::new()));
        let bytes = build_min_obcm(1);
        let cache = MapCache::new();
        let src = obc_reader::SliceSource(&bytes);
        let tables = MapTables::parse(&src).unwrap();
        let reader = Reader::new(&src, &tables, &cache);
        let mut buf = Buf::new(240, 320);
        let mut scratch = std::boxed::Box::new(obc_render::RenderScratch::new());
        app.render_frame_with_rain(Some(&mut scratch), &mut buf, &reader, None, None, Some(&snap), 240.0, 320.0, |c| {
            let (r, g, b) = rgb565_to_rgb888(c);
            Rgb888::new(r, g, b)
        });
        buf
    }

    /// A synthetic ride snapshot for the alert engine: nine 15-min frames of `intensities`
    /// anchored at the app's own wall clock, dry hourly rows.
    fn alert_snap(app: &App, intensities: &[u8]) -> crate::weather::WeatherSnapshot {
        crate::harness::support::weather_snapshot(app.wall_unix_now() as i64, intensities, None)
    }

    /// The alert engine end to end through `weather_alert_tick`: a heavy-rain snapshot fires the
    /// RAIN AHEAD card once (update-in-place on re-ticks, never a second card), persists the mark
    /// as **its own record**, stays suppressed after DISMISS, re-fires on a material escalation —
    /// and that record, round-tripped through its codec, suppresses the same storm across a boot.
    #[test]
    fn the_persisted_mark_suppresses_the_same_storm_across_a_boot() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let snap = alert_snap(&app, &[0, 10, 0, 0, 0, 0, 0, 0, 0]); // band 10 at +15 min

        // No snapshot → nothing; the engine never invents.
        app.weather_alert_tick(None);
        assert!(!matches!(app.top_screen(), Screen::WeatherAlert(_)));

        app.weather_alert_tick(Some(&snap));
        let Screen::WeatherAlert(card) = app.top_screen() else { panic!("heavy rain fires the card") };
        assert_eq!(card.kind(), crate::screen::WeatherAlertKind::Rain, "≥10 mm/h = the RAIN AHEAD face");
        assert!(serve_marks_write(&mut app).is_some(), "the fired mark is written as its own record");
        let depth = app.ui.stack.len();

        // Re-ticks with the same event: the one card updates in place, no stack growth, no
        // second persist.
        app.weather_alert_tick(Some(&snap));
        app.weather_alert_tick(Some(&snap));
        assert_eq!(app.ui.stack.len(), depth, "update-in-place, never a second card");
        assert!(drain_settings_effect(&mut app).is_none(), "a suppressed duplicate rewrites no mark");

        // DISMISS (Back pops the card): the cooldown mark keeps the same storm down.
        app.apply_gesture(Gesture::Back);
        assert!(!matches!(app.top_screen(), Screen::WeatherAlert(_)));
        app.weather_alert_tick(Some(&snap));
        assert!(!matches!(app.top_screen(), Screen::WeatherAlert(_)), "same event inside cooldown stays down");

        // Material escalation (+2 bands) breaks the cooldown and re-fires.
        let escalated = alert_snap(&app, &[0, 12, 0, 0, 0, 0, 0, 0, 0]);
        app.weather_alert_tick(Some(&escalated));
        assert!(matches!(app.top_screen(), Screen::WeatherAlert(_)), "a materially stronger storm re-fires");
        let record = serve_marks_write(&mut app).expect("the escalated mark is written too");
        app.apply_gesture(Gesture::Back);

        // Reboot: the anchors ride their own record, so the same storm stays down on a new App.
        let restored = crate::weather_alerts::decode_alert_marks(&record).expect("the record round-trips");
        let mut rebooted = App::new(AppState::new(0, 0, 1.0));
        rebooted.set_alert_marks(restored, MarksProvenance::Record);
        assert!(drain_settings_effect(&mut rebooted).is_none(), "a seed out of the record owes no write");
        rebooted.weather_alert_tick(Some(&escalated));
        assert!(
            !matches!(rebooted.top_screen(), Screen::WeatherAlert(_)),
            "the persisted mark suppresses the same storm across a boot"
        );

        // A genuinely new event fires on the rebooted device: age the persisted mark past the
        // cooldown (the equivalent of the storm having been hours ago) and the same-shaped
        // candidate is a new encounter.
        let mut aged = *rebooted.weather.alert_marks();
        aged[crate::weather_alerts::AlertClass::HeavyRain.slot()] = Some(crate::weather_alerts::AlertMark {
            onset: rebooted.wall_unix_now() as i64 - crate::weather_alerts::COOLDOWN_S - 4_000,
            pos: Some((47_000_000, 8_000_000)),
            severity: 12,
        });
        rebooted.weather.set_alert_marks(aged);
        rebooted.weather_alert_tick(Some(&escalated));
        assert!(matches!(rebooted.top_screen(), Screen::WeatherAlert(_)), "an event past the cooldown is a new alert");
    }

    /// A stored **v16** preferences blob carrying `marks` in its frozen span — what a device holds
    /// at the moment of this update. Built by doctoring the committed v16 golden, because `encode`
    /// writes v17 now and those bytes may never be re-captured.
    fn v16_blob_with(marks: crate::weather_alerts::AlertMarks) -> [u8; crate::settings::ENCODED_LEN] {
        let mut b = crate::settings::V16_FULL_BLOB;
        b[114..168].fill(0);
        crate::weather_alerts::pack_marks(&marks, &mut b[114..168]);
        let crc = crate::store_meta::crc16(&b[0..168]);
        b[168..170].copy_from_slice(&crc.to_le_bytes());
        b
    }

    /// The update does not cost the rider their anchors. A device holding a v16 blob and **no**
    /// marks record carries the blob's anchors across, rehomes them into the record once, and the
    /// storm it was already suppressing stays suppressed.
    #[test]
    fn an_upgrade_from_v16_keeps_the_anchors() {
        let mut old = App::new(AppState::new(0, 0, 1.0));
        let snap = alert_snap(&old, &[0, 10, 0, 0, 0, 0, 0, 0, 0]);
        // The last ride on the old firmware: the storm fires, and its anchor lands in the blob.
        old.weather_alert_tick(Some(&snap));
        let anchors = *old.alert_marks();
        assert!(anchors.iter().any(Option::is_some), "the old firmware really did anchor something");
        let blob = v16_blob_with(anchors);

        // The update boots: a v16 blob, and nothing in the marks record.
        let mut updated = App::new(AppState::new(0, 0, 1.0));
        updated.set_settings(crate::settings::decode(&blob).expect("the v16 blob still reads"));
        let carried = crate::settings::legacy_alert_marks(&blob).expect("the frozen span answers");
        updated.set_alert_marks(carried, MarksProvenance::LegacyBlob);
        assert_eq!(updated.alert_marks(), &anchors, "the rider's anchors came across");

        // They are rehomed into the record — once.
        let record = serve_marks_write(&mut updated).expect("the carried anchors are written to the record");
        assert_eq!(crate::weather_alerts::decode_alert_marks(&record), Some(anchors));
        assert!(drain_settings_effect(&mut updated).is_none(), "…and nothing more is owed");

        // And the same storm stays down: no duplicate card bought by the update.
        updated.weather_alert_tick(Some(&snap));
        assert!(!matches!(updated.top_screen(), Screen::WeatherAlert(_)), "the same storm stays down");
    }

    /// A storm costs 64 bytes of anchors, not 176 bytes of the rider's preferences. One firing
    /// alert offers `PersistAlertMarks` and **nothing else**: the preferences handshake is not
    /// armed, so a week of weather no longer rewrites the settings blob once per alert.
    #[test]
    fn a_firing_alert_persists_the_marks_record_and_not_the_settings_blob() {
        use crate::settings::SettingsEffect;
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let snap = alert_snap(&app, &[0, 10, 0, 0, 0, 0, 0, 0, 0]);
        let before = *app.settings();

        app.weather_alert_tick(Some(&snap));
        assert!(matches!(app.top_screen(), Screen::WeatherAlert(_)), "the card fires");
        assert_eq!(app.settings(), &before, "the preferences value is untouched");

        let effect = drain_settings_effect(&mut app).expect("the mark is owed a write");
        assert!(
            matches!(effect, SettingsEffect::PersistAlertMarks { .. }),
            "the marks record, not the blob: {effect:?}"
        );
        assert!(drain_settings_effect(&mut app).is_none(), "and no preferences write behind it");
    }

    /// An anchor never waits behind an open settings screen. The preferences debounce exists
    /// because the rider is mid-edit; a storm is not an edit, and holding its mark while a settings
    /// screen happens to be up is the behaviour this record exists to end.
    #[test]
    fn a_mark_persists_while_the_rider_is_in_the_settings_subtree() {
        use crate::settings::SettingsEffect;
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let snap = alert_snap(&app, &[0, 10, 0, 0, 0, 0, 0, 0, 0]);
        // The rider is standing in a settings screen with an edit still owed.
        let _ = app.ui.stack.push(Screen::Settings(crate::screen::SettingsScreen::new()));
        app.settings_ops.arm_save();

        // A storm fires its card over the settings screen; dismissing it lands the rider back
        // inside the subtree they never left.
        app.weather_alert_tick(Some(&snap));
        assert!(matches!(app.top_screen(), Screen::WeatherAlert(_)), "the card fires over the settings screen");
        app.apply_gesture(Gesture::Back);
        assert!(app.ui.top_is_settings(), "…and the rider is back in the subtree");

        let effect = drain_settings_effect(&mut app).expect("the mark leaves anyway");
        assert!(matches!(effect, SettingsEffect::PersistAlertMarks { .. }), "not debounced: {effect:?}");
        // The rider's own edit is still held, which is the debounce doing exactly its job.
        assert!(app.settings_ops.wants_write(false, app.ui.now_ms), "the preferences edit is owed");
        assert!(!app.settings_ops.wants_write(true, app.ui.now_ms), "…and still debounced inside the subtree");
    }

    /// The two instances mint from independent token sources, so their generations collide by
    /// construction. An answer is routed by the **record** it names, not by its token: a
    /// preferences ack carrying the marks record's own token must not clear a mark that is owed.
    #[test]
    fn a_stale_marks_outcome_cannot_clear_a_newer_mark() {
        use crate::settings::{SettingsEffect, SettingsOutcome};
        use obc_ports::SettingsSaveError;
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let snap = alert_snap(&app, &[0, 10, 0, 0, 0, 0, 0, 0, 0]);

        // A preferences write goes out first, so that source is at generation 1…
        app.settings_ops.arm_save();
        let pref = drain_settings_effect(&mut app).expect("the preferences write is owed");
        assert!(matches!(pref, SettingsEffect::PersistRevision { .. }), "preferences first: {pref:?}");
        // …and so is the marks source when the storm's write follows it. Same token by value.
        app.weather_alert_tick(Some(&snap));
        let marks = drain_settings_effect(&mut app).expect("the mark is owed a write");
        let SettingsEffect::PersistAlertMarks { token, revision } = marks else { panic!("the marks write: {marks:?}") };
        assert_eq!(token, pref.token(), "the two sources really do collide — that is the trap");

        // A *preferences*-named ack carrying that token clears the preferences write and nothing
        // else. Routed by token alone it would land on the marks record instead.
        assert!(!app.apply_settings_outcome(SettingsOutcome::Persisted { token, revision }));
        assert!(!app.settings_ops.wants_write(false, app.ui.now_ms), "the preferences write is answered");

        // The mark is still in flight: its own machine still holds the operation, so its own
        // failure is accepted and told to the rider — which a cleared machine could not do.
        let failed = SettingsOutcome::MarksPersistFailed { token, revision, error: SettingsSaveError::Backend };
        assert!(app.apply_settings_outcome(failed), "the marks write was still live and its failure is shown");
        let due = app.ui.now_ms + crate::settings::SETTINGS_RETRY_BACKOFF_MS;
        assert!(app.alert_marks_ops.wants_write(false, due), "and the anchor is still owed after the backoff");
    }

    /// A card the rider never saw is never marked as fired (review F4). `show_weather_alert`
    /// refuses at two seams — a passkey prompt on top, and a screen stack already at `MAX_DEPTH` —
    /// and either refusal used to still write the persisted dedup mark, sitting on the storm for a
    /// whole cooldown *across reboots* with nothing ever shown.
    #[test]
    fn a_refused_alert_card_writes_no_cooldown_mark() {
        use crate::weather_alerts::AlertClass;
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let snap = alert_snap(&app, &[0, 10, 0, 0, 0, 0, 0, 0, 0]); // band 10 at +15 min

        // Seam 1 — the pairing prompt outranks the card (the check `weather_alert_tick` used to
        // duplicate, now read back from the one place that decides).
        let _ = app.ui.stack.push(Screen::Passkey(crate::screen::PasskeyScreen::new(123_456)));
        app.weather_alert_tick(Some(&snap));
        assert!(matches!(app.top_screen(), Screen::Passkey(_)), "the passkey prompt is never covered");
        assert!(drain_settings_effect(&mut app).is_none());
        assert_eq!(app.alert_marks()[AlertClass::HeavyRain.slot()], None, "unseen ⇒ unmarked");
        app.ui.stack.pop();

        // Seam 2 — a full screen stack. The push has nowhere to go, so the card silently doesn't
        // open; the mark must not be written behind it.
        while app.ui.stack.len() < crate::screen::MAX_DEPTH {
            let _ = app.ui.stack.push(Screen::WeatherHourly(crate::screen::WeatherHourlyScreen::new()));
        }
        app.weather_alert_tick(Some(&snap));
        assert!(!matches!(app.top_screen(), Screen::WeatherAlert(_)), "no room on the stack: no card");
        assert!(drain_settings_effect(&mut app).is_none(), "and no persist for it");
        assert_eq!(app.alert_marks()[AlertClass::HeavyRain.slot()], None);

        // Room again: the very same storm still fires — it was never recorded as delivered.
        app.ui.stack.pop();
        app.weather_alert_tick(Some(&snap));
        assert!(matches!(app.top_screen(), Screen::WeatherAlert(_)), "the storm re-fires once there is room");
        assert!(serve_marks_write(&mut app).is_some(), "…and only now does it cost a mark");
        assert!(app.alert_marks()[AlertClass::HeavyRain.slot()].is_some());
    }

    /// The travel direction reaches the hourly rows' ink: with a tailwind-making `travel_deg`
    /// the wind arrows pick up the green tail color that the neutral (no-direction) render has
    /// nowhere on screen — the `Render::travel_deg` wiring, pinned at the pixel level.
    #[test]
    fn hourly_wind_arrows_color_route_relatively() {
        use embedded_graphics::pixelcolor::Rgb888;
        use obc_reader::rgb565_to_rgb888;

        let (r, g, b) = rgb565_to_rgb888(crate::screen::palette::ON);
        let tail_green = Rgb888::new(r, g, b);
        // Wind FROM 200° blows toward 20°; travelling at 20° that's a dead tailwind → green.
        let colored = hourly_frame(Some(20.0), 0);
        let neutral = hourly_frame(None, 0);
        assert!(colored.count(tail_green) > 0, "a tailwind row inks the arrow green");
        assert_eq!(neutral.count(tail_green), 0, "no travel direction → no head/tail claim anywhere");
    }

    /// A wet hour's millimetres are inked rain-blue, a dry hour's stay muted — counted inside the
    /// precipitation column only, since the WX17 rain icon paints its streaks in the same blue two
    /// columns to the left.
    #[test]
    fn hourly_rain_amount_is_inked_rain_blue() {
        use embedded_graphics::pixelcolor::Rgb888;
        use obc_reader::rgb565_to_rgb888;

        let (r, g, b) = rgb565_to_rgb888(crate::screen::palette::RAIN);
        let rain_blue = Rgb888::new(r, g, b);
        let blue_in_precip_column = |buf: &crate::harness::support::Buf| {
            let mut n = 0;
            for y in 0..buf.h {
                for x in 84..156 {
                    n += usize::from(buf.get(x, y) == rain_blue);
                }
            }
            n
        };

        assert!(blue_in_precip_column(&hourly_frame(None, 42)) > 0, "4.2mm reads as water");
        assert_eq!(blue_in_precip_column(&hourly_frame(None, 0)), 0, "a dry hour's 0.0mm stays muted");
    }

    /// The gust class drives the new STRONG WIND card face through the same seam.
    #[test]
    fn gust_forecast_fires_the_gust_card() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let mut snap = alert_snap(&app, &[0; 9]);
        let now = app.wall_unix_now() as i64;
        let hour = ((now - snap.valid_from) / 3_600) as usize;
        snap.hourly[hour].wind_gust_deci_ms = 220; // 22 m/s
        app.weather_alert_tick(Some(&snap));
        let Screen::WeatherAlert(card) = app.top_screen() else { panic!("dangerous gusts fire the card") };
        assert_eq!(card.kind(), crate::screen::WeatherAlertKind::Gust);
    }
    // ==================== keyed derived data (#1437) ====================
    //
    // The four rules the epic locks for a level-triggered read, exercised through the real seam:
    // a need repeats until answered, a failure answers it, an input for a key that is no longer
    // current changes nothing, and changing the subject creates a new key.

    /// A `set_rides` catalog with one ride at durable id `7`, and its detail opened.
    fn viewing_ride(ids: &[crate::CatalogObjectId]) -> App {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let summaries: heapless::Vec<crate::ride::RideSummary, 4> =
            ids.iter().map(|id| ride_summary(if *id == 7 { "First" } else { "Second" })).collect();
        app.set_rides(&summaries, ids);
        app.activity.viewed_ride = Some(0);
        app
    }

    /// The level repeats: an unanswered ride-track need is re-derived on every pass, unchanged,
    /// because nothing about it is stored.
    #[test]
    fn the_ride_track_request_repeats_until_it_is_answered() {
        let mut app = viewing_ride(&[7]);
        let first = app.derived_needs().ride_track.expect("an open detail wants its track");
        assert_eq!(first.ride, 7, "the need names the durable identity, not the catalog index");
        assert_eq!(app.derived_needs().ride_track, Some(first), "re-derived identically next pass");
        assert_eq!(ride_track_request(&app), Some(7));

        app.apply_derived(DerivedInputs::ride_track(DerivedInput::filled(first)), DerivedTargets::NONE);
        assert!(app.derived_needs().ride_track.is_none(), "answered — the level drops");
        assert_eq!(ride_track_request(&app), None);
    }

    /// A failure is a matching answer: a dead ride file costs one read, not one per pass.
    #[test]
    fn a_failed_ride_track_read_answers_the_need() {
        let mut app = viewing_ride(&[7]);
        let key = app.derived_needs().ride_track.unwrap();

        app.apply_derived(DerivedInputs::ride_track(DerivedInput::failed(key)), DerivedTargets::NONE);
        assert!(app.derived_needs().ride_track.is_none(), "a failure answers the key like a fill");
        assert_eq!(ride_track_request(&app), None, "and the level does not grind on the dead file");
    }

    /// One ride-track answer publishes both of the need's targets, from the one key — the typed path
    /// cannot leave the track page drawing an empty shape beside a filled profile.
    #[test]
    fn one_ride_track_answer_fills_the_profile_and_the_preview() {
        let mut app = viewing_ride(&[7]);
        let key = app.derived_needs().ride_track.unwrap();

        let shape = [(1, 1), (2, 2), (3, 3)];
        let targets = DerivedTargets { ride_preview: &shape, ..DerivedTargets::NONE };
        app.apply_derived(DerivedInputs::ride_track(DerivedInput::filled(key)), targets);

        assert!(app.derived_needs().ride_track.is_none(), "the need is answered");
        assert_eq!(app.catalogs.ride_preview_for(Some(key)), &shape, "…and the shape landed under the same key");
    }

    /// A stale key changes nothing. The subject moves while a read is out; when the answer finally
    /// lands it is about a ride nobody is looking at, and must not be filed under the one they are.
    #[test]
    fn a_stale_ride_track_input_changes_nothing() {
        let mut app = viewing_ride(&[7, 8]);
        let first = app.derived_needs().ride_track.unwrap();

        app.activity.viewed_ride = Some(1); // the rider moved on while the read was out
        let second = app.derived_needs().ride_track.unwrap();
        assert_ne!(first, second, "a different ride is a different need");

        app.apply_derived(DerivedInputs::ride_track(DerivedInput::filled(first)), DerivedTargets::NONE);
        assert_eq!(app.derived_needs().ride_track, Some(second), "the late answer left the live need up");
        assert_eq!(ride_track_request(&app), Some(8));
    }

    /// Changing the subject creates a new key — including *back*: returning to a ride whose answer
    /// was released asks again rather than showing what is left in the buffer.
    #[test]
    fn changing_the_viewed_ride_creates_a_new_ride_track_key() {
        let mut app = viewing_ride(&[7, 8]);
        let key = app.derived_needs().ride_track.expect("the open detail wants its track");
        app.apply_derived(DerivedInputs::ride_track(DerivedInput::filled(key)), DerivedTargets::NONE);
        assert!(app.derived_needs().ride_track.is_none());

        app.activity.viewed_ride = Some(1);
        assert_eq!(ride_track_request(&app), Some(8), "the second ride is unanswered");
        // The render pass releases the views that stopped matching the live key, every frame.
        let live = app.catalogs.ride_track_key(app.activity.viewed_ride);
        app.catalogs.drop_stale_ride_views(live);

        app.activity.viewed_ride = Some(0);
        assert_eq!(ride_track_request(&app), Some(7), "coming back asks again rather than showing stale data");
    }

    /// An abandoned in-place fill leaves the need up: `begin` invalidates the view, and only the
    /// matching `finish` answers the new key.
    #[test]
    fn an_abandoned_ride_track_fill_leaves_the_need_up() {
        let mut app = viewing_ride(&[7]);
        let before = app.derived_needs().ride_track.unwrap();

        let _buffer = app.begin_ride_profile_fill(); // …and the executor dies here
        let after = app.derived_needs().ride_track.expect("still wanted");
        assert_ne!(before, after, "starting a fill invalidates the view generation");
        assert_eq!(after.ride, before.ride, "…without pretending the subject changed");

        // The executor answers the key the need has *after* the fill — exactly what `HostLoop` does.
        app.apply_derived(DerivedInputs::ride_track(DerivedInput::filled(after)), DerivedTargets::NONE);
        assert!(app.derived_needs().ride_track.is_none(), "the completed fill answers the new key");
    }

    /// A need is not its subject: closing the Route overview ends the nav-preview level even though
    /// the route stays active, so an answer that arrives afterwards is about a question nobody is
    /// asking and must not mark the level answered on a later entry.
    #[test]
    fn an_answer_that_lands_after_the_overview_closed_is_refused() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.set_routes_with_ids(&[summary("Col")], &[10]);
        app.activity.active_route = Some(0);
        let overview = || Screen::RouteOverview(crate::screen::RouteOverviewScreen::new(0, None));
        let _ = app.ui.stack.push(overview());
        let key = app.derived_needs().nav_preview.expect("an open overview wants its shape");

        app.ui.stack.pop(); // the rider leaves while the read is out
        assert!(app.derived_needs().nav_preview.is_none(), "nothing is asking any more");
        let late = DerivedTargets { nav_preview: &[(5, 5)], ..DerivedTargets::NONE };
        app.apply_derived(DerivedInputs::nav_preview(DerivedInput::filled(key)), late);

        let _ = app.ui.stack.push(overview()); // …and comes back
        assert_eq!(app.derived_needs().nav_preview, Some(key), "the level is up again, not silently answered");
    }

    /// The nav-preview twin of the staleness rule, over the one thing identity cannot catch: an
    /// upload that replaces a stored route keeps the identity and changes the geometry.
    #[test]
    fn a_replacing_upload_stales_the_nav_preview_under_the_same_route_id() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.set_routes_with_ids(&[summary("Col")], &[10]);
        app.activity.active_route = Some(0);
        let _ = app.ui.stack.push(Screen::RouteOverview(crate::screen::RouteOverviewScreen::new(0, None)));

        let key = app.derived_needs().nav_preview.expect("an open overview wants its shape");
        assert_eq!(key.route, 10);
        app.set_nav_preview(&[(0, 0), (1, 1)]);
        assert!(!app.nav_preview_missing(), "fed once — the level retires");

        // New bytes under the same id: the identity is exactly what did not change.
        app.on_route_uploaded(10, true, None);
        let fresh = app.derived_needs().nav_preview.expect("fresh geometry wants a fresh shape");
        assert_eq!(fresh.route, key.route);
        assert_ne!(fresh.source, key.source, "the source revision moved with the bytes");

        // …and the answer produced from the old bytes can no longer land.
        let stale = DerivedTargets { nav_preview: &[(5, 5)], ..DerivedTargets::NONE };
        app.apply_derived(DerivedInputs::nav_preview(DerivedInput::filled(key)), stale);
        assert_eq!(app.derived_needs().nav_preview, Some(fresh), "the stale shape was refused");
    }
}
