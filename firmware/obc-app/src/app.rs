//! [`AppState`] is the device's view state. [`App`] is the per-frame driver both hosts run.

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
use crate::input::{Chord, Gesture};
use crate::navigator::PlanFamily;
use crate::navigator::{NavigatorIntent, NavigatorMachine, PlanPhase};
use crate::placement::define_placement_constructors;
use crate::ride::RideEntry;
use crate::route::RouteSummary;
use crate::screen::{
    self, ContextDrawerScreen, Ctx, MapScreen, MenuScreen, QuickDrawerScreen, Render, RenderFrame, Screen, WarningFlags,
};
use crate::settings::{DateTime, Settings};
use crate::ui_runtime::UiRuntime;
use crate::wall_clock::WallClock;
use crate::{DeviceStatus, Msg};
use obc_ports::{Fix, InputClock, InputSource, LocationSource, RideClock, Sensors};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    /// The camera tracks the user: every fix recenters the map.
    Follow,
    /// The camera is driven manually and ignores the user position. Fixes still move the marker.
    Free,
}

/// What Up/Down moves along while [pan mode](Pan) is in [`Move`](PanTool::Move).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanBasis {
    /// Back / ahead on the active route's cumulative-distance axis.
    Route,
    Vertical,
    Horizontal,
}

impl PanBasis {
    fn toggled_free(self) -> Self {
        match self {
            PanBasis::Vertical => PanBasis::Horizontal,
            PanBasis::Route | PanBasis::Horizontal => PanBasis::Vertical,
        }
    }

    /// The unit screen direction a positive step pans the camera toward. Route has none: it
    /// resolves against the route in [`AppState::sync_pan_route`].
    fn screen_unit(self) -> Option<(f32, f32)> {
        match self {
            PanBasis::Route => None,
            PanBasis::Vertical => Some((0.0, -1.0)),
            PanBasis::Horizontal => Some((1.0, 0.0)),
        }
    }
}

/// What Up/Down does in pan mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanTool {
    Move,
    /// Change zoom while keeping the detached camera centre fixed.
    Zoom,
}

/// Pan-mode state. While this is `Some` the camera is detached and the map rotation is frozen, so
/// a new fix or heading cannot move the map. `None` is the normal Follow map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pan {
    pub basis: PanBasis,
    pub tool: PanTool,
    /// The frozen map rotation, radians CW from north, snapshotted on entry.
    pub frozen_course_rad: f32,
    /// The inspection cursor on the route's cumulative-distance axis. It is kept while moving
    /// freely, so a return to Route resumes at the same point.
    pub route_progress_m: u32,
    /// The Free axis to come back to. Invariant: while [`basis`](Pan::basis) is a Free axis this
    /// equals it. Only [`toggle_pan_free_axis`](crate::AppState::toggle_pan_free_axis) and
    /// [`enter_pan`](crate::AppState::enter_pan) write the pair, and they write it together.
    last_free_basis: PanBasis,
    /// A route step or basis change owes one `position_at` lookup at the pre-draw boundary.
    route_camera_dirty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppState {
    /// Camera center longitude in microdegrees (1e-6°).
    pub cam_lon: i32,
    /// Camera center latitude in microdegrees (1e-6°).
    pub cam_lat: i32,
    /// Pixels per microdegree of latitude, the [`Viewport::zoom`] convention.
    pub zoom: f32,
    pub mode: CameraMode,
    /// Map orientation. `true` turns the projection so the course points to the top of the
    /// screen. Independent of [`mode`](AppState::mode).
    pub heading_up: bool,
    /// The most recent fix, or `None` before the first one.
    pub user_fix: Option<Fix>,
    /// Pan mode, or `None` on the normal Follow map.
    pub pan: Option<Pan>,
    /// Latest compass heading, degrees CW from north. It replaces the GPS course when the rider
    /// is stopped on a heading-up map or in Peak View, and is adopted only on ticks where one of
    /// those views uses it.
    pub compass_deg: Option<f32>,
    /// Terrain availability, observer framing, and summit projections. The panorama stays
    /// host-owned and is borrowed only while drawing.
    pub peak_view_profile: Option<crate::PeakViewProfile<'static>>,
    pub peak_view_peaks: [crate::PeakViewPeak; 64],
    pub peak_view_peak_count: u8,
    pub device: DeviceStatus,
    pub ble_forget_requested: bool,
    pub bond_status: crate::ble::BondStatus,
    /// Whether the open map carries a non-empty nav graph. The Detour station dims without one.
    pub has_nav_graph: bool,

    /// The Up-ahead timeline's category filter. It resets to Everything on each entry to the
    /// list. It lives here, not on the list screen, because the sheet that edits it sits above
    /// that screen on the stack.
    pub up_ahead_filter: obc_reader::PoiCategorySet,
}

impl AppState {
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
            peak_view_profile: None,
            peak_view_peaks: [crate::PeakViewPeak::EMPTY; 64],
            peak_view_peak_count: 0,
            device: DeviceStatus {
                // Stand-in until the fuel gauge feeds a real reading.
                battery_pct: 75,
                ble_link: crate::BleLink::Advertising,
                ble_paired: false,
            },
            ble_forget_requested: false,
            bond_status: crate::ble::BondStatus::Idle,
            has_nav_graph: false,

            up_ahead_filter: obc_reader::PoiCategorySet::ALL,
        }
    }

    /// Poll the location source and, in [`Follow`](CameraMode::Follow) mode with no pan,
    /// recenter the camera on the new fix. Returns the fix when one arrived this tick.
    pub fn update(&mut self, loc: &mut dyn LocationSource) -> Option<Fix> {
        let fix = loc.poll()?;
        self.user_fix = Some(fix);
        // The pan guard stops an incoming fix from moving a frozen camera.
        if self.mode == CameraMode::Follow && self.pan.is_none() {
            self.cam_lon = fix.lon;
            self.cam_lat = fix.lat;
        }
        Some(fix)
    }

    /// Project the current camera into a [`Viewport`] for a `w`×`h` pixel display.
    pub fn viewport(&self, w: f32, h: f32) -> Viewport {
        Viewport::new_rotated(w, h, self.cam_lon, self.cam_lat, self.zoom, self.course_rad())
    }

    /// The rotation (radians CW from north) the projection puts at screen-up. Shared by
    /// [`viewport`](AppState::viewport) and the pan math so the two never disagree.
    pub(crate) fn course_rad(&self) -> f32 {
        match self.pan {
            Some(pan) => pan.frozen_course_rad,
            None if self.heading_up => self.live_course_rad(),
            None => 0.0,
        }
    }

    /// The heading-up angle to freeze right now: the effective heading, or north when none is
    /// known.
    fn live_course_rad(&self) -> f32 {
        self.effective_heading_deg().map_or(0.0, |deg| deg.to_radians())
    }

    /// The rider's heading in degrees CW from north: the GPS [`course`](Fix::course) while
    /// moving, else the compass while stopped. Unlike
    /// [`live_course_rad`](AppState::live_course_rad) it does not fall back to north, so a caller
    /// that must hide rather than mislead can use the `None`.
    pub fn effective_heading_deg(&self) -> Option<f32> {
        self.user_fix.and_then(|f| f.course).or(self.compass_deg)
    }

    /// Switch to the riding view: follow the user, heading-up, and zoomed in close. The camera is
    /// seeded at `(lon, lat)` so the first frame is sensible.
    pub fn enter_riding_view(&mut self, lon: i32, lat: i32) {
        self.mode = CameraMode::Follow;
        self.heading_up = true;
        self.pan = None;
        self.cam_lon = lon;
        self.cam_lat = lat;
        self.zoom = zoom_for_mpp(RIDING_MPP);
    }

    /// Enter pan mode: detach the camera, freeze the orientation, and start in Move. A loaded
    /// route makes route-relative movement the default.
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

    pub fn exit_pan(&mut self) {
        self.pan = None;
        self.mode = CameraMode::Follow;
        self.recenter_on_user();
    }

    pub fn recenter_on_user(&mut self) {
        if let Some(fix) = self.user_fix {
            self.cam_lon = fix.lon;
            self.cam_lat = fix.lat;
        }
    }

    /// Advance the pan mode ring on a Select tap: Route Move, Free Move, Zoom, Route Move. No-op
    /// when not panning.
    pub fn cycle_pan_mode(&mut self, has_route: bool) {
        let Some(pan) = self.pan.as_mut() else { return };
        match (pan.tool, pan.basis) {
            (PanTool::Move, PanBasis::Route) => pan.basis = pan.last_free_basis,
            (PanTool::Move, _) => pan.tool = PanTool::Zoom,
            (PanTool::Zoom, _) => {
                pan.tool = PanTool::Move;
                if has_route {
                    pan.basis = PanBasis::Route;
                    pan.route_camera_dirty = true;
                }
            }
        }
    }

    /// Toggle Free Vertical and Free Horizontal (Select hold). No-op in Route and Zoom.
    pub fn toggle_pan_free_axis(&mut self) {
        if let Some(pan) = self.pan.as_mut() {
            if pan.tool == PanTool::Zoom || pan.basis == PanBasis::Route {
                return;
            }
            pan.basis = pan.basis.toggled_free();
            pan.last_free_basis = pan.basis;
        }
    }

    /// Apply `steps` from Up/Down to the active pan tool. Free movement travels [`PAN_STEP_PX`]
    /// screen pixels; route movement converts the same visual distance to ground metres and
    /// defers its geometry lookup to [`sync_pan_route`](Self::sync_pan_route).
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

    /// Resolve a dirty route inspection cursor to its coordinate. It runs once at the pre-draw
    /// boundary, the only place that holds the active [`RouteReader`].
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

    /// Shift the camera centre by a screen-space pixel offset. It reuses [`Viewport::to_map`] on
    /// a zero-sized viewport: the screen centre cancels out of the inverse projection, so this
    /// needs no display dimensions.
    fn pan_by_pixels(&mut self, dx: f32, dy: f32) {
        let vp = Viewport::new_rotated(0.0, 0.0, self.cam_lon, self.cam_lat, self.zoom, self.course_rad());
        let (lon, lat) = vp.to_map(dx, dy);
        self.cam_lon = lon;
        self.cam_lat = lat;
    }
}

/// Ground metres per pixel to zoom to when a route loads.
const RIDING_MPP: f32 = 0.5;

/// Camera travel per Up/Down step in pan mode, in screen pixels, so panning is finer when zoomed in.
pub const PAN_STEP_PX: f32 = 40.0;

/// Zoom multiplier per Up/Down step, shared by the Follow map and pan mode's Zoom tool.
pub(crate) const ZOOM_STEP: f32 = 1.2;
/// Zoom clamps in pixels per microdegree of latitude.
pub(crate) const MIN_ZOOM: f32 = 1e-6;
pub(crate) const MAX_ZOOM: f32 = 1e4;

pub(crate) fn step_zoom(mut zoom: f32, steps: i32, min: f32, max: f32) -> f32 {
    let step = if steps >= 0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
    for _ in 0..steps.unsigned_abs() {
        zoom *= step
    }
    zoom.clamp(min, max)
}

/// Capacity of one frame's gesture buffer. One frame yields at most one gesture per raw event
/// (the input queue holds 8) plus one long-press, so this never overflows.
pub const GESTURE_BUF: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockTrust {
    /// No real time source stamped the clock this boot, so the value is the persisted set-point.
    /// It is display-only: no stamps and no deletions.
    Untrusted,
    /// A GPS fix stamped the clock this boot.
    Gps,
    /// A BLE `setClock` from the phone stamped the clock this boot.
    Ble,
}

pub const POSITION_FIX_FRESH_MS: u32 = 30_000;

const NO_FIX_FLOOR_MS: u32 = 5_000;
const NO_FIX_INTERVALS: u32 = 3;
const BATTERY_POLL_MS: u32 = 30_000;

/// Tick-local timing and sampling state that no product domain or screen owns.
#[derive(Debug, Default)]
struct TickState {
    last_battery_poll_ms: Option<u32>,
    temp_c: Option<f32>,
    last_fix_ms: Option<u32>,
    pending_terrain: Option<(i32, i32)>,
}

impl TickState {
    const fn new() -> Self {
        TickState { last_battery_poll_ms: None, temp_c: None, last_fix_ms: None, pending_terrain: None }
    }

    fn battery_poll_due(&mut self, now_ms: u32) -> bool {
        let due = self.last_battery_poll_ms.is_none_or(|last| now_ms.wrapping_sub(last) >= BATTERY_POLL_MS);
        if due {
            self.last_battery_poll_ms = Some(now_ms);
        }
        due
    }

    fn has_live_fix(&self, now_ms: u32, settings: &Settings) -> bool {
        let window = (settings.fix_interval_s as u32 * 1_000 * NO_FIX_INTERVALS).max(NO_FIX_FLOOR_MS);
        self.last_fix_ms.is_some_and(|last| now_ms.wrapping_sub(last) <= window)
    }

    #[cfg(test)]
    fn assert_boot_state(&self) {
        assert!(self.last_battery_poll_ms.is_none() && self.temp_c.is_none(), "nothing sampled at boot");
        assert!(self.last_fix_ms.is_none() && self.pending_terrain.is_none(), "no fix or terrain request at boot");
    }
}

pub struct App {
    /// The camera, orientation and last-fix state. Public so the host can pan and zoom it
    /// directly.
    pub state: AppState,
    /// The operating mode, viewed ride, and small UI requests outside a product domain.
    pub activity: Activity,
    /// The resident route, ride and trip catalogs keyed by durable object ids, plus the
    /// identity-keyed view caches. It is the one owner of the id-to-summary pairing and of every
    /// rescan-remap invariant.
    pub(crate) catalogs: CatalogState,
    tick_state: TickState,
    /// The UI plane: the screen stack, the fused input plane, the map-plane clock, repaint
    /// accumulation and wake scheduling, the idle-return policy, and the card scheduler.
    pub(crate) ui: UiRuntime,
    /// The persisted device settings, seeded from the host's store at boot.
    settings: Settings,
    /// The live wall clock: the [`settings.clock`](Settings::clock) set-point advanced by elapsed
    /// monotonic millis. There is no RTC, so this is how a static readout ticks.
    wall_clock: WallClock,
    clock_trust: ClockTrust,
    pub recorder: crate::recorder::RecorderMachine,

    /// The Navigator domain: route following and caches, the rider's undelivered plan requests,
    /// per-family phase, and operation token. It is the only writer of route guidance and of
    /// [`mode`](App::mode)'s two search levels.
    pub(crate) navigator: NavigatorMachine,
    pub(crate) metadata: crate::metadata::MetadataMachine,
    pub(crate) easier: crate::easier::State,
    /// The one owner of what heavy work may run now and what the rider is looking at: the two
    /// search levels Navigator writes, the transfer level, and the Recalculating banner's edge
    /// bit. Every reader of "a search is live" derives from it; nothing keeps a second copy.
    pub(crate) mode: CoreMode,
    /// The settings-persistence machine: the dirty revision, the subtree debounce, the retry
    /// backoff and the stale-answer rule.
    pub(crate) settings_ops: crate::settings::SettingsMachine,

    /// The DFU domain: the single most-recent-wins update phase and its token.
    pub(crate) dfu: DfuState,
    pub(crate) bond: crate::ble::BondMachine,
    /// The free-space refresh, its token, and the figure the System screen prints.
    pub(crate) storage: StorageInfo,
    /// The DeviceCore coordinator's own state: every cross-domain connection, the levels a stage
    /// detects an edge against, the current [`Capabilities`](crate::device_core::Capabilities)
    /// and the re-entrancy guard. Nothing here decides a product rule.
    pub(crate) pass: crate::device_core::pass::PassState,
    /// The running firmware version string, fed by the host at boot. Resident because the System
    /// screen draws on a frame with no `Reader`.
    fw_version: heapless::String<32>,
    /// The loaded map's display name, fed on map load. Empty until a map loads.
    map_name: heapless::String<24>,
    /// The loaded map's OBCM format version, the right half of the `Map` row. `0` until a map loads.
    map_obcm_version: u8,
    /// Whether this platform's panel has a controllable light, declared once by the host at
    /// composition. `false` removes the quick drawer's brightness control, and it is the default,
    /// so a platform that says nothing offers no control it has no port for.
    backlight_available: bool,
}

/// Cap on the computed route's shape-preview polyline. The host decimates the planned polyline to
/// at most this many points, which keeps a fixed ~512 B buffer here instead of a route-sized one.
pub const NAV_PREVIEW_MAX: usize = 64;

impl App {
    /// Build the app straight onto the live map: stack `[Home, Map]`, no route loaded. The GUI
    /// and the device boot through [`new_idle`](App::new_idle) instead.
    pub fn new(state: AppState) -> Self {
        let mut app = Self::new_idle(state);
        app.open_map_first();
        app
    }

    /// The map-first tail both map-first constructors share.
    fn open_map_first(&mut self) {
        self.activity = Activity::new(Mode::Riding);
        let _ = self.ui.stack.push(Screen::Map(MapScreen::new()));
    }

    define_placement_constructors!(
        /// Build the app at the device's real power-on state: the Home screensaver, Idle, no
        /// route loaded.
        pub fn new_idle(state: AppState);
        /// Build the idle power-on [`App`] in place at `slot`, so firmware can construct the
        /// resident `App` without materializing it on the stack. Each KB-scale component is
        /// written by its own placement constructor. The render scratch stays host-owned.
        pub unsafe fn init_idle;
        fields {
            state: state,
            activity: Activity::new(Mode::Idle),
            catalogs: CatalogState::new() => CatalogState::init_in_place,
            tick_state: TickState::new(),
            ui: UiRuntime::new() => UiRuntime::init_in_place,
            settings: Settings::default(),
            // The clock starts from the default set-point; the host re-stamps the persisted value.
            wall_clock: WallClock::new(Settings::default().local_clock()),
            clock_trust: ClockTrust::Untrusted,
            recorder: crate::recorder::RecorderMachine::new() => crate::recorder::RecorderMachine::init_in_place,
            navigator: NavigatorMachine::new() => NavigatorMachine::init_in_place,
            metadata: crate::metadata::MetadataMachine::new(),
            easier: crate::easier::State::new(),
            mode: CoreMode::new(),
            settings_ops: crate::settings::SettingsMachine::new(),
            dfu: DfuState::new(),
            bond: crate::ble::BondMachine::new(),
            storage: StorageInfo::new(),
            pass: crate::device_core::pass::PassState::new(),
            fw_version: heapless::String::new(),
            map_name: heapless::String::new(),
            map_obcm_version: 0,
            backlight_available: false,
        }
    );

    /// Build the map-first [`App`] in place at `slot`: initialise the idle state, then drop
    /// straight onto the live Map. Firmware uses it to put the map on glass before buttons exist.
    ///
    /// # Safety
    /// Same contract as [`init_idle`](App::init_idle).
    pub unsafe fn init_map(slot: *mut App, state: AppState) {
        // SAFETY: caller's contract. `init_idle` fully initialises the slot, so `&mut *slot` is sound.
        unsafe { Self::init_idle(slot, state) };
        unsafe { &mut *slot }.open_map_first();
    }

    /// Assert the [`new_idle`](App::new_idle) boot state field by field. The destructure is
    /// exhaustive, so a field added to the plan must state its boot value here too.
    #[cfg(test)]
    fn assert_idle_boot_state(&self, state: AppState) {
        let App {
            state: camera,
            activity,
            catalogs,
            tick_state,
            ui,
            settings,
            wall_clock,
            clock_trust,
            recorder,

            navigator,
            metadata,
            easier: _,
            mode,
            settings_ops,

            dfu,
            bond,
            storage,
            pass,
            fw_version,
            map_name,
            map_obcm_version,
            backlight_available,
        } = self;
        assert_eq!(*camera, state, "the camera state is preserved verbatim");
        assert_eq!(activity.mode, Mode::Idle, "boots Idle, not Riding");
        catalogs.assert_boot_state();
        tick_state.assert_boot_state();
        ui.assert_boot_state();
        assert_eq!(*settings, Settings::default(), "the defaults until the store answers");
        assert_eq!(*wall_clock, WallClock::new(Settings::default().local_clock()), "the default set-point");
        assert_eq!(*clock_trust, ClockTrust::Untrusted, "a persisted set-point is display-only this boot");

        assert!(settings_ops.is_empty(), "settings Clean at revision 0");

        navigator.assert_boot_state();
        metadata.assert_boot_state();
        recorder.assert_boot_state();
        assert_eq!(*mode, CoreMode::new(), "nothing searching, nothing streaming, no banner shown");
        dfu.assert_boot_state();
        assert_eq!(bond.status(), crate::ble::BondStatus::Idle);
        storage.assert_boot_state();
        assert_eq!(*pass, crate::device_core::pass::PassState::new(), "no connection wired, no pass in flight");
        assert!(fw_version.is_empty() && map_name.is_empty(), "the host has identified nothing yet");
        assert_eq!(*map_obcm_version, 0, "no map format known yet");
        assert!(!*backlight_available, "no host has claimed a panel light yet");
    }

    pub fn tick(&mut self, clock: RideClock, sensors: Sensors, route: Option<&RouteReader>) {
        self.advance_inputs(clock, sensors, route);
    }

    /// Apply the world to the app: the sensor ports, the fix and its derived readouts, and the
    /// repaint edges they imply. The sensor half of [`tick`](App::tick).
    pub(crate) fn advance_inputs(&mut self, clock: RideClock, sensors: Sensors, route: Option<&RouteReader>) {
        let now_ms = clock.0;
        // Sensor freshness is judged on the `RideClock`, so remember it here. The stat tiles
        // render later against the map-plane clock, which is GPX-playback time in the simulator
        // and would blank them to `--` seconds into a replay.
        self.recorder.note_sensor_clock(now_ms);
        // A paused rider stops the totals without ending the session, so Recorder is told, not
        // asked.
        let riding = self.activity.mode == Mode::Riding;
        // Read the freeze once, before anything below can move the stack: while a planner run
        // holds the arena, this tick must not advance route-match progress.
        let frozen = self.reroute_freeze_active();
        // The once-per-load route and session sync is Navigator's. A change there repaints the
        // map even on a frame with no fresh fix.
        if self.navigator.sync_route_state(route) {
            self.ui.map_dirty = true;
        }
        if let Some(bike) = self.navigator.take_loaded_bike_type(route).filter(|b| *b != self.settings.bike_type) {
            self.settings.bike_type = bike;
            self.settings_ops.note_edited();
        }
        // A detour commit queues a seam re-anchor because the commit handler owns no
        // `RouteReader`. Anchor it before this tick's fresh fix, then re-derive the guidance.
        if self.navigator.apply_pending_seam(route) {
            if let Some(route) = route {
                self.update_active_climb(route);
                self.update_next_waypoint(route);
            }
            self.ui.map_dirty = true;
        }

        let Sensors { loc, altimeter, temperature, clock, compass, fuel, hr, power, cadence } = sensors;
        // Battery charge from the PMIC gauge, on a slow cadence. Nothing here marks a repaint:
        // Home's render key carries the level, so the riding views are not woken every 30 s.
        if self.tick_state.battery_poll_due(now_ms) {
            if let Some(soc) = fuel.and_then(|f| f.poll()) {
                self.state.device.battery_pct = soc;
            }
        }
        // Barometric altitude. Polled before the fix so a point logged this tick carries the
        // freshest altitude.
        if let Some(altimeter) = altimeter {
            if let Some(alt) = altimeter.poll() {
                self.recorder.record_altitude(alt, riding);
            }
        }
        // Ambient temperature. Stored off `AppState` so it never gates a map redraw.
        if let Some(temperature) = temperature {
            if let Some(c) = temperature.poll() {
                self.tick_state.temp_c = Some(c);
            }
        }
        // BLE sensors. Drained beside the altimeter so `record_fix` sees this tick's samples.
        // `Some` only on a fresh reading; a dropped strap stops reporting and the staleness gate
        // expires the last value.
        if let Some(hr) = hr {
            if let Some(bpm) = hr.poll() {
                self.recorder.record_hr(bpm, now_ms);
            }
        }
        if let Some(power) = power {
            if let Some(watts) = power.poll() {
                self.recorder.record_power(watts, now_ms);
            }
        }
        if let Some(cadence) = cadence {
            if let Some(rpm) = cadence.poll() {
                self.recorder.record_cadence(rpm, now_ms);
            }
        }
        // GPS can establish UTC before a position fix is available.
        if let Some(t) = clock.and_then(|c| c.poll()) {
            // GPS carries no timezone, so `None` leaves the persisted offset untouched.
            self.stamp_clock(t.utc, t.second, None, ClockTrust::Gps);
        }
        // A fresh fix only: a dropout must not re-run the matcher or double-count the totals.
        if let Some(fix) = self.state.update(loc) {
            // Stamp fix freshness against the map-plane clock, the one the banner's staleness
            // check reads. It is off `AppState`, so a stationary fix forces no redraw.
            self.tick_state.last_fix_ms = Some(self.ui.now_ms);
            let base = screen::base_index(&self.ui.stack);
            if let Some(Screen::PeakView(screen)) = self.ui.stack.get_mut(base) {
                if screen.needs_position() {
                    screen.set_status(crate::peak_view::runtime::Status::Building(0));
                    self.ui.map_dirty = true;
                }
            }
            // Arm the map-referenced altimeter's one terrain read for this fix. Nothing is
            // sampled here: the host drains it after this tick, at the fix cadence.
            self.tick_state.pending_terrain = Some((fix.lat, fix.lon));
            if let Some(route) = route {
                // The Recalculating freeze pauses exactly this: the matcher. A search can replace
                // the geometry that progress is measured along, so advancing it under a map
                // nobody redraws is drift the rider cannot see. The ride keeps accumulating.
                if frozen {
                    // The cursor stands still while fixes keep coming, so the next match must not
                    // be judged against a one-fix-wide forward window. Arm the wide re-lock.
                    self.navigator.note_unmatched_fix();
                } else {
                    self.navigator.match_fix(fix, route);
                }
                self.update_active_climb(route);
                self.update_next_waypoint(route);
            }

            // No write happens here: the staged sample leaves as a `RecorderEffect::Append`, so
            // nothing on the fix path touches a medium. `true` means the staging buffer was full
            // and the log lost a point.
            if self.recorder.record_fix(fix, now_ms, riding) {
                self.on_warning(WarningFlags::REC_ERROR);
            }
        }
        // Electronic compass. Polled after the fix so it sees this tick's movement state, and
        // adopted only when it would drive the orientation: a stopped rider on a heading-up map,
        // or Peak View, which uses the same heading chain whatever the map preference is.
        if let Some(compass) = compass {
            if let Some(heading) = compass.poll() {
                let stopped = self.state.user_fix.and_then(|f| f.course).is_none();
                let peak_view_active = matches!(self.ui.stack.last(), Some(Screen::PeakView(_)));
                let map_uses_compass = self.state.heading_up && self.state.pan.is_none();
                if (stopped && map_uses_compass) || peak_view_active {
                    let changed = self.state.compass_deg != Some(heading);
                    self.state.compass_deg = Some(heading);
                    if changed && peak_view_active {
                        self.ui.map_dirty = true;
                    }
                }
            }
        }
        // Nothing below this line asks for a repaint. The riding views declare the facts they
        // draw, so the pass compares them at its own boundary, for the visible screens only.
    }

    /// Give the map-referenced altimeter its one terrain read for the latest GPS fix. Call it
    /// once per host pass, immediately after [`tick`](App::tick).
    ///
    /// Returns whether a sample was taken: `false` on any pass with no fresh fix, which is most
    /// of them. [`tick`](App::tick) arms the request on a fresh fix only, so the read happens at
    /// the fix cadence and never per frame. A terrain sample can be an SD read, which has no
    /// business on the render path.
    ///
    /// It is not part of [`tick`](App::tick): `Sensors` is `obc-ports` vocabulary and terrain is
    /// the map. That also keeps the source's `&mut` out of the fix path, where the board holds it
    /// as a `.bss` `&'static mut` shared with the planner.
    pub fn sample_terrain(&mut self, elev: &mut dyn ElevationSource) -> bool {
        let Some((lat, lon)) = self.tick_state.pending_terrain.take() else { return false };
        if let Some(map_m) = elev.sample(lat, lon) {
            self.recorder.record_map_elevation(map_m);
        }
        true
    }
    /// Recompute Navigator's active climb from the freshly-matched progress, then apply the
    /// App-plane consequence of a transition: the host auto-switch off the same edge.
    fn update_active_climb(&mut self, route: &RouteReader) {
        if let Some((prev, next)) = self.navigator.update_active_climb(route) {
            // No repaint request: the active climb is in the Statistics and Climb render keys.
            self.apply_climb_auto_switch(prev, next);
        }
    }

    /// Recompute Navigator's next waypoint from the freshly-matched progress.
    fn update_next_waypoint(&mut self, route: &RouteReader) {
        // No repaint request: the next waypoint is in the Map and Statistics render keys.
        self.navigator.update_next_waypoint(route);
    }

    /// The Auto-mode screen follow, driven off the climb entry/exit edge in
    /// [`update_active_climb`](App::update_active_climb).
    ///
    /// On entry, [`Auto`](crate::settings::ClimbMode::Auto) mode switches the top screen to the
    /// Climb screen only when it is exactly Map or Statistics, so a rider in a menu or a chooser
    /// is never yanked out. On exit it replaces a Climb screen anywhere in the stack with Map,
    /// whatever the mode, so a later return cannot show a stale "No climb" panel. It replaces
    /// rather than pushes, because Climb is a sibling of the riding views, not an overlay.
    fn apply_climb_auto_switch(&mut self, prev: Option<usize>, next: Option<usize>) {
        let top_is = |app: &Self, want: fn(&Screen) -> bool| app.ui.stack.last().is_some_and(want);
        match (prev, next) {
            (None, Some(_))
                if self.settings.climb_mode == crate::settings::ClimbMode::Auto
                    && top_is(self, |s| matches!(s, Screen::Map(_) | Screen::Statistics(_))) =>
            {
                if let Some(top) = self.ui.stack.last_mut() {
                    *top = Screen::Climb(crate::screen::ClimbScreen::new());
                }
            }
            (Some(_), None) => {
                if let Some(climb) = self.ui.stack.iter_mut().rfind(|s| matches!(s, Screen::Climb(_))) {
                    *climb = Screen::Map(MapScreen::new());
                }
            }
            _ => {}
        }
    }

    /// Whether the base (lowest opaque) screen draws the map. A render-on-demand host polls this
    /// to skip the whole map pipeline on a non-map frame: no `Reader` build, `None` to
    /// [`render_scene_map_photo_timed`](App::render_scene_map_photo_timed), and a menu redraw with zero map I/O.
    pub fn base_draws_map(&self) -> bool {
        self.ui.base_draws_map()
    }

    /// The one region a hold step repaints on a static map base, such as the Route overview's
    /// Delete row, or `None`. Only there does a hold repaint without a map render: the region lies
    /// outside the page's map band, and that page draws no live map. A live map base answers
    /// `None` and keeps deferring its redraws while a hold charges.
    pub fn hold_fill_region(&self) -> Option<Rectangle> {
        if !self.ui.base_draws_map() || self.ui.base_draws_live_map() || !self.top_wants_hold_fill() {
            return None;
        }
        let (w, h) = (i32::from(self.ui.frame_size.0), i32::from(self.ui.frame_size.1));
        self.ui.stack.last().and_then(|s| s.hold_fill_region(w, h))
    }

    /// Whether the Recalculating freeze is engaged: a host planner run is live and the base
    /// screen would draw the map. While it is, a render-on-demand host must skip the map redraw,
    /// leave the last frame on the glass, and paint only [`render_overlay`](App::render_overlay).
    /// [`tick`](App::tick) stops advancing route-match progress for the same span.
    ///
    /// The board also reads it to decide whether the nav arm of the scratch arena may be claimed:
    /// the map plane must be quiet before a search overwrites the render scratch.
    pub fn reroute_freeze_active(&self) -> bool {
        self.mode.frozen(self.ui.base_draws_map())
    }

    /// Whether the visible place choices are still being prepared.
    pub fn find_preparing(&self) -> bool {
        use crate::find_place::{Action, State};
        matches!(self.top_screen(), Screen::FindPlace(screen) if screen.choices())
            && (matches!(self.ui.find.action, Action::Refresh | Action::Preview(_))
                || matches!(self.ui.find.state, State::Start | State::Querying | State::Planning))
    }

    /// A preview mode change waiting for the previous planner and catalog owners.
    pub fn assistant_route_pending(&self) -> bool {
        matches!(self.top_screen(), Screen::VisitReview(screen) if screen.pending_target.is_some())
    }

    fn planning_banner(&self) -> Option<Msg> {
        if self.find_preparing() {
            Some(Msg::AssistantFinding)
        } else if self.assistant_route_pending()
            || self.reroute_freeze_active()
            || (matches!(self.top_screen(), Screen::VisitReview(_))
                && self.assistant_review_status() == crate::navigator::ReviewStatus::Planning)
        {
            Some(Msg::AssistantPlanningRoute)
        } else {
            None
        }
    }

    fn planning_banner_phase(&self) -> u8 {
        if self.planning_banner().is_some() {
            1 + (self.ui.now_ms / 1_000 % 3) as u8
        } else {
            0
        }
    }

    /// What the device is busy with, ranked and payload-free. Named apart from
    /// [`mode`](App::mode), which is the rider's activity, Idle or Riding. A search outranks a
    /// transfer because it is the one the rider waits on; admission reads the levels, not this
    /// ranking, so one cannot hide behind the other.
    pub fn core_mode(&self) -> ModeState {
        self.mode.state()
    }

    /// The proof that the map plane is quiesced, minted from this app's own state. `None` when a
    /// search must not take the arena yet. The board writes
    /// `app.nav_arena_precondition().ok_or(…)?`, so the gate cannot be called without it.
    pub fn nav_arena_precondition(&self) -> Option<crate::arena_gate::MapQuiesced> {
        self.mode.nav_precondition(self.base_draws_map())
    }

    /// The proof that a cable upload may take the arena's staging arm: the transfer card is up
    /// and no search holds the nav arm.
    pub fn usb_stage_precondition(&self) -> Option<crate::arena_gate::TransferReady> {
        self.mode.usb_precondition(self.map_transfer_card_up())
    }

    /// Whether the frame needs the streamed-map [`Reader`] built and passed to
    /// [`render_scene_map_photo_timed`](App::render_scene_map_photo_timed): a superset of
    /// [`base_draws_map`](App::base_draws_map). Map-base screens always do, and the POI list and
    /// POI detail screens do until their one-shot reads resolve in the pre-draw prepare pass.
    /// After that they draw from frozen state, so the host skips the build again.
    pub fn base_needs_reader(&self) -> bool {
        self.ui.base_needs_reader()
    }

    /// Whether the frame this pass renders draws the sheet and nothing else: a drawer covers a
    /// base that does not recess, on a host that declared a
    /// [resident frame](App::set_resident_frame), so the base's draw is skipped.
    ///
    /// A host asks so it can say so: a frame that skipped the base is not a screen redraw and
    /// must not report itself as one.
    pub fn sheet_only(&self) -> bool {
        self.ui.sheet_only()
    }

    /// Whether there is a current GPS fix at `now_ms`: one has been accepted and is no older
    /// than the staleness window. `false` before the first fix and once the signal drops.
    pub fn has_live_fix(&self, now_ms: u32) -> bool {
        self.tick_state.has_live_fix(now_ms, &self.settings)
    }

    /// Feed the running firmware version string. The host calls this once at boot with its
    /// build's `git describe` tag. It is truncated to the 32-byte field, never ellipsized.
    pub fn set_fw_version(&mut self, version: &str) {
        self.fw_version.clear();
        for ch in version.chars() {
            if self.fw_version.push(ch).is_err() {
                break;
            }
        }
    }

    /// Feed the loaded map's display name and OBCM format version on map load. The System
    /// screen's `Map` row reads it as `name · vN`.
    pub fn set_map_info(&mut self, name: &str, obcm_version: u8) {
        self.ui.poi_scratch.cancel();
        self.ui.corridor_scratch.cancel();
        self.map_name.clear();
        for ch in name.chars() {
            if self.map_name.push(ch).is_err() {
                break;
            }
        }
        self.map_obcm_version = obcm_version;
    }

    /// Declare whether this platform's panel has a controllable light. The host asks its
    /// [`Backlight`](obc_ports::Backlight) port once at composition and states the answer here.
    /// `false` removes the quick drawer's brightness control: a control the hardware cannot
    /// honour is worse than no control.
    pub fn set_backlight_available(&mut self, available: bool) {
        if self.backlight_available != available {
            self.backlight_available = available;
            self.ui.map_dirty = true;
        }
    }

    pub fn backlight_available(&self) -> bool {
        self.backlight_available
    }

    /// Replace the resident route catalog from the host's store, carrying each route's durable
    /// object id (`ids` parallel to `summaries`), then remap every held catalog index by id.
    /// Clones up to [`MAX_ROUTES`](crate::MAX_ROUTES) entries; any beyond that are ignored.
    ///
    /// The remap is the live-catalog contract: a rescan that inserts or removes a route
    /// re-points the active route, the caches keyed on it, an open chooser, a menu selection and
    /// a pending swap at the same route by id. A vanished route unloads navigation, clamps a
    /// menu selection, and turns a preview subject into its screen's missing-route path.
    pub fn set_routes_with_ids(&mut self, summaries: &[RouteSummary], ids: &[crate::CatalogObjectId]) {
        let len = summaries.len().min(ids.len()).min(crate::MAX_ROUTES);
        if self.catalogs.routes() == &summaries[..len] && self.catalogs.route_ids() == &ids[..len] {
            return;
        }
        // The old-id snapshot drives the remap of everything held outside `CatalogState`.
        let old_ids = self.catalogs.replace_routes(summaries, ids);
        self.remap_route_indices(&old_ids);
        self.ui.map_dirty = true;
    }
    /// Mark unaccepted immutable candidates in the current complete catalog projection.
    pub fn route_unaccepted(&self, index: usize) -> bool {
        self.navigator.route_unaccepted(index)
    }

    /// Saved Routes omits these rows; navigation and recovery keep the complete catalog.
    pub fn set_internal_routes(&mut self, mask: u64) {
        if self.navigator.internal_routes() != mask {
            self.navigator.set_internal_routes(mask);
            for screen in self.ui.stack.iter_mut() {
                if let Screen::RouteMenu(menu) = screen {
                    menu.remap_routes(&Some, self.catalogs.trips(), self.catalogs.route_len(), mask);
                }
            }
            self.ui.map_dirty = true;
        }
    }
    pub fn set_unaccepted_routes(&mut self, mask: u64) {
        if self.navigator.unaccepted_routes() != mask {
            self.navigator.set_unaccepted_routes(mask);
            self.ui.map_dirty = true;
        }
    }
    /// Re-point every held catalog index after the catalog was replaced: old index, to its id in
    /// `old_ids`, to that id's new index, or `None` if the route vanished.
    fn remap_route_indices(&mut self, old_ids: &[crate::CatalogObjectId]) {
        let App { catalogs, ui, navigator, .. } = self;
        let remap = |i: usize| -> Option<usize> { catalogs.remap_route(old_ids, i) };

        navigator.remap_route_keys(&remap);
        navigator.remap_detour_route(&remap);
        navigator.remap_review_keys(&remap);

        // The Route menu also takes the re-resolved trips and the new route count, so it can
        // follow its highlight into the regrouped list.
        let new_len = catalogs.route_len();
        let trips = catalogs.trips();
        for s in ui.stack.iter_mut() {
            match s {
                Screen::RouteMenu(m) => m.remap_routes(&remap, trips, new_len, navigator.internal_routes()),
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

    pub fn routes(&self) -> &[RouteSummary] {
        self.catalogs.routes()
    }

    /// Each catalog entry's durable object id, pairwise with [`routes`](App::routes).
    pub fn route_ids(&self) -> &[crate::CatalogObjectId] {
        self.catalogs.route_ids()
    }

    /// The active route's catalog index, or `None` when no route is loaded. A host syncs its
    /// route store's active bytes from this each pass; it never writes the field.
    /// The name a ride opened now is saved under: the active route's, except that a ride on a
    /// Ride-to-start splice takes the name of the route it leads to.
    pub fn ride_name(&self) -> Option<&str> {
        let active = self.active_route_index()?;
        let ids = self.route_ids();
        let name = |i: usize| self.routes().get(i).map(|r| r.name.as_str());
        let origin = match self.navigator.lead_in() {
            Some(lead) if ids.get(active) == Some(&lead.splice) => ids.iter().position(|&id| id == lead.route),
            _ => None,
        };
        origin.and_then(name).or_else(|| name(active))
    }

    pub fn active_route_index(&self) -> Option<usize> {
        self.navigator.route_state().active_route
    }

    /// The rider's matched along-route progress, in meters. It freezes while off-route.
    pub fn progress_m(&self) -> u32 {
        self.navigator.route_state().progress_m
    }

    /// Whether the matcher currently places the rider outside the route corridor.
    pub fn off_route(&self) -> bool {
        self.navigator.route_state().off_route
    }

    /// Activate the route at catalog index `idx`, bounds-checked against the resident catalog, so
    /// a host never writes Navigator's active route directly. An out-of-range index clears it.
    pub fn activate_route(&mut self, idx: usize) {
        self.navigator.set_active_route((idx < self.catalogs.route_len()).then_some(idx));
        self.ui.map_dirty = true;
    }

    /// Replace the resident trip catalog from the host's store. Each
    /// [`TripInput`](crate::trip::TripInput) carries the trip's durable id, name and stage route
    /// ids; the app resolves them against the current route catalog into a
    /// [`TripSummary`](crate::trip::TripSummary), dropping dangling refs. Call it after the
    /// routes are set so the stage ids resolve; a later
    /// [`set_routes_with_ids`](App::set_routes_with_ids) re-resolves them in place.
    pub fn set_trips(&mut self, trips: &[crate::trip::TripInput]) {
        let trips = &trips[..trips.len().min(crate::trip::MAX_TRIPS)];
        if self.catalogs.trips().len() == trips.len()
            && self.catalogs.trips().iter().zip(trips).all(|(old, input)| {
                *old == crate::trip::TripSummary::resolve(input, self.catalogs.routes(), self.catalogs.route_ids())
            })
        {
            return;
        }
        self.catalogs.set_trips(trips);
        self.ui.map_dirty = true;
    }

    /// The resident trip catalog: the grouped-route folders.
    pub fn trips(&self) -> &[crate::trip::TripSummary] {
        self.catalogs.trips()
    }

    /// What a ride that starts now records: the current bike type, and the trip day when the
    /// loaded route is one.
    pub(crate) fn ride_origin(&self) -> crate::RideOrigin {
        let route = self.active_route_index().and_then(|i| self.route_ids().get(i).copied());
        let route =
            route.map(|id| self.navigator.lead_in().filter(|lead| lead.splice == id).map_or(id, |lead| lead.route));
        crate::RideOrigin {
            bike: self.settings.bike_type,
            trip: route.and_then(|route| crate::trip::trip_day(self.trips(), route)),
        }
    }

    /// The device's trip progress records, at most one per trip key.
    pub fn trip_progress(&self) -> &[crate::trip::TripProgress] {
        self.metadata.progress()
    }

    /// The active trip's next day: the one the host reads a [`DayJoin`](crate::trip::DayJoin) for.
    pub fn next_trip_day(&self) -> Option<(&crate::trip::TripSummary, u16)> {
        crate::trip::next_trip_day(self.trips(), self.metadata.progress()).map(|(trip, day, _)| (trip, day))
    }

    /// Where the active trip's next day meets the day before, read after the catalog read that fed
    /// the trips and the progress records.
    pub fn set_day_join(&mut self, join: Option<crate::trip::DayJoin>) {
        if self.metadata.day_join() != join {
            self.metadata.set_day_join(join);
            self.ui.map_dirty = true;
        }
    }

    /// Replace the trip progress records with the ones a complete catalog read found.
    pub fn set_trip_progress(&mut self, records: impl IntoIterator<Item = crate::trip::TripProgress>) {
        self.metadata.set_progress(records);
        self.ui.map_dirty = true;
    }

    /// The record a [`WriteProgress`](crate::metadata::MetadataEffect::WriteProgress) writes.
    pub fn trip_progress_payload(
        &self,
        token: crate::device_core::OperationToken<crate::device_core::MetadataTag>,
    ) -> Option<&crate::trip::TripProgress> {
        self.metadata.progress_payload(token)
    }

    /// A saved ride on a trip day moves that trip's progress to where the ride ended.
    pub(crate) fn note_trip_finish(&mut self) {
        use crate::trip::TripPosition;
        let Some(ridden) = self.recorder.ride_stats().trip else { return };
        let Some(trip) = self.trips().iter().find(|t| t.key == ridden.key()) else { return };
        let day = u16::from(ridden.day_index());
        let Some(&route) = trip.stage_ids.get(usize::from(day)) else { return };
        let old = trip.progress_in(self.metadata.progress());
        let active_index = self.active_route_index();
        let active = active_index.and_then(|i| self.route_ids().get(i).copied());
        // After a reset the adopted lead-in is gone, but a built day is still the internal route
        // the record and the line facts describe: its rest starts where the record stands.
        let built =
            active_index.is_some_and(|i| active != Some(route) && self.navigator.internal_routes() & (1 << i) != 0);
        let lead =
            self.navigator.lead_in().filter(|lead| Some(lead.splice) == active && lead.route == route).or_else(|| {
                match trip.load_day(day, old, self.metadata.day_join().as_ref()) {
                    crate::trip::DayLoad::Rest { from_m, to_m, join_m } if built => Some(crate::navigator::LeadIn {
                        splice: active?,
                        route,
                        lead_m: to_m - from_m,
                        join_m,
                        rest_from_m: Some(from_m),
                    }),
                    _ => None,
                }
            });
        let mut at = match lead.map(|lead| lead.position(self.progress_m())) {
            Some(Err(metres)) if day > 0 => {
                TripPosition { day: day - 1, route: trip.stage_ids[usize::from(day) - 1], metres }
            }
            Some(Ok(metres)) => TripPosition { day, route, metres },
            _ if active == Some(route) => TripPosition { day, route, metres: self.progress_m() },
            _ => TripPosition { day, route, metres: 0 },
        };
        // Metres measured on geometry that a re-upload replaced during the ride mean nothing on the
        // new one.
        if self.metadata.replaced_during_ride(at.route) {
            at.metres = 0;
        }
        let today = if self.clock_trusted() { (self.wall_clock.unix_now(self.ui.now_ms) / 86_400) as u16 } else { 0 };
        let record = trip.finish(old, day, at, today);
        let trips = self.catalogs.trips();
        self.metadata.owe_progress(record, |key| trips.iter().any(|t| t.key == key));
    }

    /// A route the open ride's trip day stands on was replaced: note it, so the Finish does not
    /// carry metres from the old geometry.
    fn note_ride_route_replaced(&mut self, id: crate::CatalogObjectId) {
        let Some(ridden) = self.recorder.ride_stats().trip else { return };
        let Some(trip) = self.trips().iter().find(|t| t.key == ridden.key()) else { return };
        let day = usize::from(ridden.day_index());
        if trip.stage_ids.get(day.saturating_sub(1)..=day).is_some_and(|days| days.contains(&id)) {
            self.metadata.note_replaced_during_ride(id);
        }
    }

    /// The open ride's footer facts. The trip name is read from the trip catalog as the footer is
    /// written, so a ride continued after a reset names its trip too.
    pub fn ride_stats(&self) -> obc_route::RideStats {
        let mut stats = self.recorder.ride_stats();
        if let Some(trip) = stats.trip.and_then(|day| self.trips().iter().find(|t| t.key == day.key())) {
            stats.trip_name = obc_formats::ride::Name::new(&trip.name);
        }
        stats
    }

    /// Whether the route at catalog index `idx` is filed into a trip. A filed route shows only
    /// inside its folder.
    pub fn route_filed(&self, idx: usize) -> bool {
        self.catalogs.route_filed(idx)
    }

    /// Replace the host's paired ride snapshot, newest first. Keep the newest
    /// [`UI_RIDES_CAP`](crate::UI_RIDES_CAP) summaries visible, and re-point open screens by
    /// durable id across the rescan.
    pub fn set_rides(&mut self, entries: &[RideEntry]) {
        let entries = &entries[..entries.len().min(crate::UI_RIDES_CAP)];
        if self.catalogs.rides() == entries {
            return;
        }
        // Screen indices follow the durable identity through each rescan.
        let old_ids = self.catalogs.replace_rides(entries);
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
    /// Apply an exact durable archive row after the complete catalog and metadata reads succeed.
    /// The caller must finish the refresh under its unchanged store scope before policy can run.
    pub fn set_ride_archive_proof(&mut self, id: crate::CatalogObjectId, timestamp: u32) {
        self.catalogs.set_ride_archive_proof(id, timestamp);
    }

    /// The resident ride catalog, as the Rides screen lists it.
    pub fn rides(&self) -> &[RideEntry] {
        self.catalogs.rides()
    }

    /// Borrow the app's one resident ride-profile buffer for an in-place host fill. It
    /// invalidates the ride-track view: until a keyed answer for the post-fill key lands, the
    /// level re-fires, so an abandoned fill leaves a need up, not a half-written buffer marked
    /// answered.
    pub fn begin_ride_profile_fill(&mut self) -> &mut Profile {
        self.catalogs.begin_ride_profile_fill()
    }

    /// Open the on-glass DFU check flow from a remote BLE `installFw` request: push the checking
    /// card and post [`DfuAction::Scan`](crate::activity::DfuAction). It never installs; only the
    /// rider's Select press on the confirm screen arms an install.
    ///
    /// Returns `true` when the flow opened. `false` defers: the board keeps the request pending
    /// and retries next pass, so an inconvenient moment delays the card rather than dropping it.
    /// The one deferral that never retries is a confirmed shutdown, where there is no next pass
    /// and a card would take [`power_off_requested`](App::power_off_requested) back to `false`.
    pub fn open_remote_dfu_check(&mut self) -> bool {
        let dfu_screen_up = self.ui.stack.iter().any(|s| {
            matches!(s, Screen::DfuCheck(_) | Screen::DfuConfirm(_) | Screen::DfuProgress(_) | Screen::DfuError(_))
        });
        if self.passkey_card_up()
            || self.ui.hold_charging()
            || dfu_screen_up
            || self.dfu.request_pending()
            || self.recorder.recording()
            || self.power_off_requested()
        {
            return false;
        }
        self.dfu.admit_intent(crate::dfu::DfuIntent::ScanRequested);
        // Through `apply`, not `stack.push`: pushing raw would step around the rule every other
        // arrival obeys, that nothing lands on top of a drawer.
        screen::apply(
            &mut self.ui.stack,
            screen::Transition::Push(Screen::DfuCheck(crate::screen::DfuCheckScreen::new())),
        );
        self.ui.map_dirty = true;
        true
    }

    /// Debug bench: start a route plan from `from` to `to` (both `(lon, lat)` µdeg) exactly as
    /// the POI create-route confirm does, so the host steps the resumable router with the same
    /// spinner and render cadence the rider sees. Only the `debug-uart` build wires it; no UI
    /// path reaches it. Returns `false` without changing pending state while a plan is active.
    pub fn debug_start_nav(&mut self, from: (i32, i32), to: (i32, i32), name: &str) -> bool {
        // Reject a repeat before touching the request slot: once the host has drained the first
        // request, overwriting the resident planner would orphan its allocation.
        if self.ui.stack.iter().any(|s| matches!(s, Screen::NavPlanning(_))) {
            return false;
        }
        self.admit_navigator_intent(NavigatorIntent::PlanRoute(crate::activity::NavRequest::new(from, to, name)));
        // Raw, and deliberately: a bench line is not an arrival a rider can produce, so the
        // "nothing lands on top of a drawer" rule does not apply here.
        let _ = self.ui.stack.push(Screen::NavPlanning(crate::screen::NavPlanningScreen::new(name)));
        self.ui.map_dirty = true;
        true
    }

    /// Debug only: arm an install exactly as the confirm screen's press does, for the board's
    /// physical `dfu-install` VCOM command, which skips the confirm.
    ///
    /// It names the intent to [`DfuState`](crate::dfu::DfuState) rather than reaching for the
    /// executor, so the debug path and the rider's path produce the same effect under the same
    /// operation token. Only the domain can mint one.
    pub fn debug_request_dfu_install(&mut self) {
        self.dfu.admit_intent(crate::dfu::DfuIntent::InstallRequested);
    }

    /// Debug and snapshot only: engage the Recalculating freeze as if the host had just begun a
    /// planner run, taking the same seam a drained plan does.
    ///
    /// The freeze's visible state is not reachable from a scripted headless run: the flows that
    /// start a plan leave an opaque planning screen as the base, and the one gesture that puts a
    /// map base back under a live search also cancels the plan. The simulator's `--freeze` flag
    /// drives this so the banner can be snapshotted over a live map. It stands in for a
    /// [`Route`](PlanFamily::Route) run, so a stray detour edge cannot release it.
    pub fn debug_set_plan_live(&mut self, live: bool) {
        if self.navigator.debug_set_plan_live(live, &mut self.mode) {
            self.ui.map_dirty = true;
        }
    }

    /// Drop everything derived from the active route's geometry. This is the whole-App seam, and
    /// the only thing route-replacing paths should call.
    ///
    /// The UI also drops its next-category cache and corridor snapshot: their along-route
    /// distances belong to the old geometry even when the index and frozen progress are unchanged.
    pub(crate) fn drop_route_derived_state(&mut self) {
        self.navigator.drop_route_derived_state();
        self.ui.next_ahead.invalidate();
        self.ui.corridor_scratch.invalidate();
    }

    /// Hand one rider request to Navigator, and repaint.
    ///
    /// The map is dirtied on any navigation intent because every screen that produces one is
    /// changing what the rider is looking at. A plan start dirties nothing extra: the executor is
    /// about to stop redrawing the map, and the banner's edge is the engaged level.
    pub(crate) fn admit_navigator_intent(&mut self, intent: NavigatorIntent) {
        let planned = self.navigator.detour_planned();
        self.navigator.admit_intent(intent);
        self.sync_detour_preview(planned);
        self.ui.map_dirty = true;
    }

    /// Cancel a plan the rider can no longer answer. A plan is a question, and the screens it is
    /// asked from are the only place to drop it: the chooser and its preview for a detour, the
    /// planning spinner for a route search. A question that outlives them holds the planner arena,
    /// and its release keeps a result nobody accepted — a line drawn over the active route, or a
    /// computed route left in the store.
    ///
    /// It reads the stack shape, because the transitions that drop a descent whole — `OverRoot`
    /// under the Assistant chord and under the drawer's settings row, and the escape chord — never reach the screens in
    /// it, so none of them can cancel on its own way out. An adopted result is not a question:
    /// the splice truncates its own screens off once the rider has the spliced route.
    ///
    /// The route family's preview and failure phases are not read here. They belong to the
    /// Assistant's review, whose own release is reconciled from the stack by
    /// [`prepare_find`](App::prepare_find); cancelling the plan under it would leave the review
    /// machine holding a checkpoint for a plan that no longer exists.
    fn release_unreachable_plans(&mut self) {
        use crate::navigator::PlanFamily;
        if self.navigator.plan_awaits_rider(PlanFamily::Detour)
            && !self.ui.stack.iter().any(|s| {
                matches!(s, Screen::Detour(_) | Screen::DetourPreview(_))
                    || matches!(s, Screen::NavPlanning(p) if matches!(p.kind(), crate::screen::PlanKind::Approach | crate::screen::PlanKind::Day))
            })
        {
            self.admit_navigator_intent(NavigatorIntent::CancelDetour);
        }
        if self.navigator.route_search_running()
            && !self
                .ui
                .stack
                .iter()
                .any(|s| matches!(s, Screen::NavPlanning(p) if p.kind() == crate::screen::PlanKind::Nav))
        {
            self.admit_navigator_intent(NavigatorIntent::CancelPlan);
        }
    }

    /// Drop the detour preview polyline when Navigator drops the plan it previews.
    ///
    /// The shape is derived from the plan and is drawn over the still-active route, so a preview
    /// of a detour that no longer exists is a line to nowhere. `was_planned` is the level from
    /// before the intent, so this fires on the falling edge only.
    fn sync_detour_preview(&mut self, was_planned: bool) {
        if was_planned && !self.navigator.detour_planned() {
            self.catalogs.clear_detour_preview();
        }
    }

    /// Consume a typed [`NavigatorOutcome`](crate::navigator::NavigatorOutcome). The token is the
    /// whole admission test: a cancelled or superseded operation refuses its own late answer.
    pub(crate) fn apply_navigator_outcome(&mut self, outcome: crate::navigator::NavigatorOutcome) {
        use crate::navigator::{NavigatorError, NavigatorOutcome};
        if !self.navigator.accepts(&outcome) {
            return;
        }
        match outcome {
            NavigatorOutcome::ReviewReady { .. } if self.assistant_measuring() => {
                self.navigator.measured();
                self.end_plan(PlanFamily::Route, PlanPhase::Idle);
            }
            NavigatorOutcome::ReviewReady { .. } => {
                let Some(preview) = self.assistant_preview() else { return };
                let index = self.route_ids().iter().position(|&id| id == preview.source.object);
                self.navigator.review_index(index);
                self.navigator.reviewed(preview);
                self.end_plan(PlanFamily::Route, PlanPhase::PreviewReady);
            }
            NavigatorOutcome::PlanFinished { route, .. } => self.land_route_plan(Ok(route)),
            NavigatorOutcome::DetourFinished { preview, .. } => self.land_detour_plan(Ok(preview)),
            NavigatorOutcome::DetourCommitted { route, .. } => self.land_detour_commit(Ok(route)),
            NavigatorOutcome::Failed { error, .. } => {
                if self.assistant_review_context().is_some() {
                    self.navigator.review_failed(error);
                    self.end_plan(PlanFamily::Route, PlanPhase::Failed);
                    return;
                }
                // The resource failures have no tier of their own and land on the generic card.
                let error = match error {
                    NavigatorError::Plan(error) => error,
                    NavigatorError::Workspace
                    | NavigatorError::Store
                    | NavigatorError::SourceChanged
                    | NavigatorError::Movement
                    | NavigatorError::Unavailable
                    | NavigatorError::DurabilityUnknown => obc_route::nav::NavError::NoPath,
                };
                match self.navigator.live_family() {
                    Some(PlanFamily::Detour) if self.navigator.detour_committing() => {
                        self.land_detour_commit(Err(error))
                    }
                    Some(PlanFamily::Detour) => self.land_detour_plan(Err(error)),
                    _ => self.land_route_plan(Err(error)),
                }
            }
            NavigatorOutcome::ReleaseUnresolved { .. } => {
                self.navigator.review_failed(NavigatorError::DurabilityUnknown);
                self.navigator.released_unresolved(&mut self.mode);
                self.ui.map_dirty = true;
            }
            NavigatorOutcome::Released { .. } => {
                if self.navigator.released(&mut self.mode) || self.ui.find.state == crate::find_place::State::Planning {
                    self.ui.map_dirty = true;
                }
            }
            NavigatorOutcome::Cancelled { .. } => {
                let family = self.navigator.live_family().unwrap_or(PlanFamily::Route);
                self.end_plan(family, PlanPhase::Idle);
            }
            NavigatorOutcome::Acquired { .. } => {
                self.navigator.progressed(crate::navigator::PlannerProgress::Searching)
            }
            NavigatorOutcome::Stepped { progress, .. } => self.navigator.progressed(progress),
        }
    }

    /// Consume a typed [`DfuOutcome`](crate::dfu::DfuOutcome), behind the token that says this
    /// answer is still the phase being waited for.
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
            // An abandoned phase leaves the rider where they were; a failure card for work
            // never done would be worse.
            DfuOutcome::Cancelled { .. } => {}
        }
    }

    /// Note a terminal planner answer for `family` and repaint the map the freeze held still.
    ///
    /// The map is dirtied because it held still for the whole search and has a fix, a route or a
    /// new geometry to catch up on. Idempotent: several release edges can land for one run.
    fn end_plan(&mut self, family: PlanFamily, phase: PlanPhase) {
        if self.navigator.note_answer(family, phase, &mut self.mode) {
            self.ui.map_dirty = true;
        }
    }

    /// The UI's reaction to Navigator finishing a route plan: land it in the planning screen, or
    /// drop it. Navigator has already decided that this answer is the one being waited for.
    fn land_route_plan(&mut self, result: Result<crate::CatalogObjectId, obc_route::nav::NavError>) {
        use obc_route::nav::NavError;
        // The run is over whatever happens below, including a late answer whose planning screen
        // the rider already cancelled away.
        self.end_plan(PlanFamily::Route, if result.is_ok() { PlanPhase::Active } else { PlanPhase::Failed });
        let Some(i) = self.ui.stack.iter().position(|s| matches!(s, Screen::NavPlanning(_))) else {
            return;
        };
        // A missing id degrades to the generic failure tier.
        let resolved = result.and_then(|id| self.catalogs.route_index_of(id).ok_or(NavError::NoPath));
        let screen = match resolved {
            Ok(idx) => {
                // New bytes can sit under a same-id reserved file, so drop everything derived
                // from the old geometry and let the matcher re-lock.
                self.drop_route_derived_state();
                // Activate for the preview; `prev` restores whatever was loaded on cancel.
                let prev = self.navigator.replace_active_route(idx);
                // Every plan starts preview-less: a re-route commits new bytes under the same
                // id, so an old shape must never survive into the new overview.
                self.catalogs.invalidate_nav_preview();
                Screen::RouteOverview(crate::screen::RouteOverviewScreen::computed(idx, prev))
            }
            // Exhaustion is the device's honest "too far"; everything else is the generic tier.
            Err(NavError::Exhausted) => Screen::NavFail(crate::screen::NavFailScreen::too_far()),
            Err(_) => Screen::NavFail(crate::screen::NavFailScreen::not_found()),
        };
        self.ui.stack[i] = screen;
        self.ui.map_dirty = true;
    }

    /// The detour plan's answer: land it in the detour planning screen. Success replaces it with
    /// the preview, failure with the fail card. A late answer whose planning screen is gone is
    /// dropped, and the stale preview slot cleared.
    fn land_detour_plan(&mut self, result: Result<crate::host::DetourPreview, obc_route::nav::NavError>) {
        use obc_route::nav::NavError;
        // The run is over — see `land_route_plan` for the late-answer case.
        self.end_plan(PlanFamily::Detour, if result.is_ok() { PlanPhase::PreviewReady } else { PlanPhase::Failed });
        // A lead-in has no preview to look at: its leg goes straight to the splice.
        if self.navigator.lead_leg().is_some() {
            match (self.lead_in_planning(), result) {
                (Some(_), Ok(preview)) => {
                    self.navigator.note_lead_preview(&preview);
                    self.admit_navigator_intent(NavigatorIntent::CommitDetour)
                }
                (Some(slot), Err(_)) => self.land_lead_in_failure(slot),
                (None, _) => self.admit_navigator_intent(NavigatorIntent::CancelDetour),
            }
            return;
        }
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

    /// The splice's answer: re-adopt the spliced route and land back on the riding view, or
    /// surface the failure on the preview with the old route fully intact.
    ///
    /// Success order matters: drop every cache derived from the old geometry, point
    /// `active_route` at the spliced route, queue the seam re-anchor, then truncate the detour
    /// flow off the stack so the rider lands on the riding view they left.
    fn land_detour_commit(&mut self, result: Result<crate::CatalogObjectId, obc_route::nav::NavError>) {
        self.navigator.note_commit(result.is_ok());
        let resolved = result.and_then(|id| self.catalogs.route_index_of(id).ok_or(obc_route::nav::NavError::NoPath));
        if let Some(leg) = self.navigator.lead_leg() {
            match (self.lead_in_planning(), resolved) {
                (Some(slot), Ok(idx)) if matches!(leg, obc_route::Leg::Rest { .. }) => self.land_day(slot, idx),
                (Some(_), Ok(idx)) => self.ride_approach(idx),
                (Some(slot), Err(_)) => self.land_lead_in_failure(slot),
                // The rider escaped the spinner, so the ride they asked for is no longer wanted:
                // the cancel makes the release retract the publication.
                (None, _) => {
                    self.catalogs.clear_detour_preview();
                    self.admit_navigator_intent(NavigatorIntent::CancelDetour);
                }
            }
            return;
        }
        match resolved {
            Ok(idx) => {
                let anchor = self.ui.stack.iter().find_map(|s| match s {
                    Screen::DetourPreview(p) => Some(p.anchor_m()),
                    _ => None,
                });
                self.drop_route_derived_state();
                // The splice commits new geometry under the same route identity, so the derived
                // keys must move with the bytes.
                self.catalogs.note_commit();
                self.navigator.set_active_route(Some(idx));
                self.navigator.request_seam(idx, anchor.unwrap_or(0));
                self.catalogs.clear_detour_preview();
                if let Some(i) = self.ui.stack.iter().position(|s| matches!(s, Screen::Detour(_))) {
                    self.ui.stack.truncate(i.max(1)); // never below the Home root
                }
                self.ui.map_dirty = true;
            }
            Err(_) => {
                // The splice failed before anything was adopted, so the old route and session
                // are untouched.
                for s in self.ui.stack.iter_mut() {
                    if let Screen::DetourPreview(p) = s {
                        p.set_commit_failed();
                    }
                }
                self.ui.map_dirty = true;
            }
        }
    }

    /// The stack slot of the lead-in spinner, while it waits for its plan or its splice.
    fn lead_in_planning(&self) -> Option<usize> {
        self.ui.stack.iter().position(|s| {
            matches!(s, Screen::NavPlanning(p) if matches!(p.kind(), crate::screen::PlanKind::Approach | crate::screen::PlanKind::Day))
        })
    }

    /// Ride to start is spliced: start the ride on the approach and the route, as START RIDE does.
    fn ride_approach(&mut self, idx: usize) {
        let origin = self.ui.stack.iter().find_map(|s| match s {
            Screen::StartAway(prompt) => self.catalogs.route_ids().get(prompt.route()).copied(),
            _ => None,
        });
        if let (Some(origin), Some(&splice)) = (origin, self.catalogs.route_ids().get(idx)) {
            self.navigator.adopt_lead_in(splice, origin);
        }
        self.drop_route_derived_state();
        self.catalogs.note_commit();
        self.catalogs.clear_detour_preview();
        // The spliced route starts at the fix the leg was planned from.
        let Some((lon, lat)) = self.catalogs.routes().get(idx).map(|r| (r.start_lon, r.start_lat)) else { return };
        self.navigator.load_route(idx);
        let ride = screen::begin_riding_session(&mut self.state, &mut self.activity, &mut self.recorder, lon, lat);
        screen::apply(&mut self.ui.stack, ride);
        self.ui.map_dirty = true;
    }

    /// The day is spliced from the rest of the day before: the spinner at `slot` becomes the
    /// spliced route's detail, as the day's own detail would be.
    fn land_day(&mut self, slot: usize, idx: usize) {
        let day = self.navigator.route_state().active_route;
        if let (Some(route), Some(&splice)) =
            (day.and_then(|i| self.catalogs.route_ids().get(i).copied()), self.catalogs.route_ids().get(idx))
        {
            self.navigator.adopt_lead_in(splice, route);
        }
        self.drop_route_derived_state();
        self.catalogs.note_commit();
        let prev = self.ui.stack.iter().rev().find_map(|s| match s {
            Screen::RideStart(start) => Some(start.prev_active()),
            _ => None,
        });
        self.navigator.set_active_route(Some(idx));
        self.ui.stack[slot] = Screen::RouteOverview(crate::screen::RouteOverviewScreen::new(idx, prev.flatten()));
        self.ui.map_dirty = true;
    }

    /// A lead-in found no way: drop the spinner at `slot`. Ride to start leaves the prompt it came
    /// from with Join nearest and Cancel.
    fn land_lead_in_failure(&mut self, slot: usize) {
        self.ui.stack.truncate(slot.max(1));
        if let Some(Screen::StartAway(prompt)) = self.ui.stack.last_mut() {
            prompt.set_no_route();
        }
        self.ui.map_dirty = true;
    }

    /// Feed whether the loaded map carries a non-empty nav graph, once at map open. It gates the
    /// ride menu's Detour station and the chooser.
    pub fn set_map_nav_graph(&mut self, present: bool) {
        self.state.has_nav_graph = present;
    }

    /// Hand in the planned detour's decimated polyline, keyed to the active route the detour was
    /// planned against.
    pub fn set_detour_preview(&mut self, pts: &[(i32, i32)]) {
        self.catalogs.set_detour_preview(pts, self.active_route_index());
        self.ui.map_dirty = true;
    }

    /// Whether a Route overview is up without its route-shape preview. It is the host's per-pass
    /// cue to decimate the active route's polyline and hand it to
    /// [`set_nav_preview`](App::set_nav_preview). The fill runs once per overview entry, not per
    /// pass: it goes `false` the moment the preview is in or the overview is gone.
    pub fn nav_preview_missing(&self) -> bool {
        self.derived_needs().nav_preview.is_some()
    }

    /// Hand in the previewed route's decimated shape polyline: at most [`NAV_PREVIEW_MAX`]
    /// `(lon, lat)` µdeg points, decimated host-side. It is keyed to the previewed route's
    /// durable identity, the revision its bytes last changed at, and the view generation, so a
    /// route change, a re-plan over the same id, or a committed detour all stale it.
    pub fn set_nav_preview(&mut self, pts: &[(i32, i32)]) {
        use crate::device_core::derived::{DerivedInput, DerivedInputs, DerivedTargets};
        let Some(key) = self.derived_needs().nav_preview else { return };
        let input = DerivedInput::filled(key);
        self.apply_derived(
            DerivedInputs::nav_preview(input),
            DerivedTargets { nav_preview: pts, ..DerivedTargets::NONE },
        );
    }

    /// Feed the host's BLE link snapshot. The board's BLE plane distils its `ble::state` into
    /// this each pass; the simulator injects it from the control panel. No BLE crate type
    /// crosses the boundary.
    ///
    /// A change in the link phase or the paired flag dirties the map only where the state is
    /// drawn, so an unchanged status, fed every pass, repaints nothing. A passkey going `Some`
    /// opens the passkey card over whatever is up, and its clearing closes it. The card defers
    /// while a hold is charging, because yanking the hold target out mid-charge would break the
    /// confirm; the sweep lands it on the next pass.
    pub fn set_ble_status(&mut self, status: crate::ble::BleStatus) {
        let changed = (self.state.device.ble_link, self.state.device.ble_paired) != (status.link, status.paired);
        self.state.device.ble_link = status.link;
        self.state.device.ble_paired = status.paired;
        // An explicit repaint request, not a render key: this seam is a host feeder that can
        // ring between two passes. It is gated on the base screen drawing the glyph.
        if changed && self.ui.indicator_visible() {
            self.ui.map_dirty = true;
        }
        self.ui.cards.set_passkey(status.passkey);
        self.sweep_cards();
    }

    /// Whether the passkey card is currently up. A route-upload popup is dropped, not queued,
    /// while the card shows.
    pub fn passkey_card_up(&self) -> bool {
        self.ui.passkey_card_up()
    }

    /// Run the one [`CardScheduler`](crate::card_scheduler::CardScheduler) sweep with the facts
    /// it needs. Called once per pass, and again after any host fact is posted, so an arriving
    /// card lands in the same frame unless a policy rule defers it.
    fn sweep_cards(&mut self) {
        self.ui.run_card_sweep(&self.catalogs, self.recorder.recording());
        if self.ui.stack.iter().any(|s| matches!(s, Screen::Journey(_))) || self.ui.find.resume_offer {
            self.ui.find.review = self.assistant_review_status();
            self.ui.find.resume_route = self
                .assistant_checkpoint()
                .and_then(|saved| self.route_ids().iter().position(|&id| id == saved.route.object))
                .and_then(|index| u8::try_from(index).ok());
        }
        let arrival = self.visit_arrival_pending();
        let accepted = self.assistant_review_status() == crate::navigator::ReviewStatus::Accepted;
        if accepted {
            self.ui.find.resume_offer = false;
        }
        let hold = self.ui.hold_charging();
        self.ui.map_dirty |=
            self.ui.cards.reconcile_journey(&mut self.ui.stack, hold, arrival, self.ui.find.resume_offer, accepted);
    }

    /// The update domain's terminal answer: post it for the DFU wait on the stack. The scheduler
    /// drops it when that wait is gone.
    pub(crate) fn post_dfu_landing(&mut self, landing: DfuLanding) {
        self.ui.cards.post_dfu(landing);
        self.sweep_cards();
    }

    /// Post this boot's one-time update verdict for the toast.
    pub(crate) fn post_boot_update(&mut self, result: BootUpdate) {
        self.ui.cards.post_update(result);
        self.sweep_cards();
    }

    /// Offer explicit cleanup after a route upload ran out of storage.
    pub fn offer_route_cleanup(&mut self, store: crate::device_core::StoreIdentity) {
        if self.ui.stack.iter().any(|s| matches!(s, Screen::RouteCleanup(_))) {
            return;
        }
        let utc = self.clock_trusted().then(|| self.wall_unix_now());
        screen::apply(
            &mut self.ui.stack,
            screen::Transition::Push(Screen::RouteCleanup(screen::RouteCleanupScreen::new(utc, store))),
        );
        self.ui.cancel_holds();
        self.ui.map_dirty = true;
    }

    pub fn set_map_transfer(&mut self, state: Option<crate::screen::MapTransfer>) {
        self.ui.cards.set_map_transfer(state);
        self.sweep_cards();
    }

    /// Whether the map-transfer card is currently up. This is how a host observes the seam.
    pub fn map_transfer_card_up(&self) -> bool {
        self.ui.map_transfer_card_up()
    }

    /// The live BLE pairing passkey, or `None` when not pairing, as last fed to
    /// [`set_ble_status`](App::set_ble_status).
    pub fn ble_passkey(&self) -> Option<u32> {
        self.ui.cards.passkey_level()
    }

    /// Feed the host's per-slot sensor status: the connection phase, battery and live tick for
    /// HR, power and cadence, pushed each pass. Up to
    /// [`SENSOR_SLOTS`](crate::settings::SENSOR_SLOTS) slots are copied. A change repaints only
    /// while the Sensors screen is up, so the steady feed costs nothing elsewhere.
    pub fn set_sensor_status(&mut self, status: &[crate::sensors::SensorStatus]) {
        self.ui.set_sensor_status(status);
    }

    /// Feed the sensors discovered while the scan-list screen runs a scan. It replaces the
    /// resident list wholesale, up to [`SCAN_HITS_MAX`](crate::sensors::SCAN_HITS_MAX); an empty
    /// slice clears it. A change while the scan screen is up repaints it.
    pub fn set_sensor_scan_hits(&mut self, hits: &[crate::sensors::SensorScanHit]) {
        self.ui.set_sensor_scan_hits(hits);
    }

    /// Whether the rider is on the scan-list screen and a scan should run. The host reads the
    /// level each pass: while `true` it keeps a discovery scan running and feeds the hits back;
    /// when it falls it clears the app scan list.
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
        let active_id = self.active_route_index().and_then(|i| self.catalogs.route_id_at(i));
        let active_replace = replaced && active_id == Some(id);
        if replaced {
            self.note_ride_route_replaced(id);
            self.invalidate_current_visit(id);
            // New bytes under an unchanged identity: the id is exactly what did not change, so
            // every derived key must move with the bytes.
            self.catalogs.note_commit();
        }
        if active_replace {
            // Same index and id, but new bytes. The remap preserves same-id state, and a
            // replace is the one case where that would carry stale state onto new geometry.
            self.drop_route_derived_state();
            self.navigator.owe_bike_type();
            self.ui.map_dirty = true; // the drawn route line + progress changed under the rider
        }
        self.ui.cards.post_upload(PendingUpload::Route(UploadEvent { id, active_replace, elevation }));
        self.sweep_cards();
    }

    /// A committed trip upload: the advisory prompt, for a fresh trip only. There is nothing to
    /// adopt, because a trip is a folder of already-committed routes, each of which raised its
    /// own event. A replace is a trip edit the user just made, so it is silent; a card per click
    /// would be the parade this event exists to kill.
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

    pub fn debug_stack_len(&self) -> usize {
        self.ui.stack.len()
    }

    /// Whether any drawer is on the stack: observability for the "nothing lands on top of a
    /// drawer" rule.
    pub fn debug_stack_has_overlay(&self) -> bool {
        self.ui.stack.iter().any(|s| s.is_overlay())
    }

    /// Offer one journaled ride recovered at boot to the rider.
    ///
    /// The host calls this after it has reconstructed `continuation` from the durable sample
    /// prefix. The first successful call restores the accumulators and roots the UI at the
    /// Continue / hold-to-Discard card. Repeated calls are no-ops, so a level-style recorder
    /// status can be fed every pass without reopening the decision. An already-tracking app
    /// refuses: recovery is a boot decision and never replaces a live session.
    pub fn offer_recovered_ride(&mut self, continuation: crate::RideContinuation) -> bool {
        if !self.recorder.offer_recovery(crate::recorder::RideRecoveryState::Resumable) {
            return false;
        }
        self.recorder.restore_continuation(continuation);
        self.activity.mode = Mode::Idle;
        self.navigator.suspend_for_recording_recovery();
        self.raise_ride_recovery()
    }

    /// Surface a durable recording an executor could not attach to a session, named by what is
    /// wrong with it. Logical damage on a readable catalog offers the one hold-guarded Discard;
    /// [`RideDamage::Catalog`](crate::RideDamage::Catalog) offers no action at all.
    pub fn offer_damaged_ride(&mut self, damage: crate::RideDamage) -> bool {
        if !self.recorder.offer_recovery(crate::recorder::RideRecoveryState::for_damage(damage)) {
            return false;
        }
        self.recorder.restore_continuation(crate::RideContinuation::default());
        self.activity.mode = Mode::Idle;
        self.navigator.suspend_for_recording_recovery();
        self.raise_ride_recovery()
    }

    /// Root the UI at the recovery card in whatever mode Recorder's state names, and cancel any
    /// hold in flight so the card's guarded row starts from zero. It is the one place the card
    /// is raised, so the two boot offers and the pass's re-raise cannot drift. Re-rooting is also
    /// what repaints it: the card is `Static`-keyed. `false` means the state names no decision.
    pub(crate) fn raise_ride_recovery(&mut self) -> bool {
        let Some(mode) = crate::screen::RecoveryMode::of(self.recorder.recovery()) else {
            return false;
        };
        screen::apply(
            &mut self.ui.stack,
            screen::Transition::Root(Screen::RideRecovery(crate::screen::RideRecoveryScreen::new(mode))),
        );
        self.ui.map_dirty = true;
        self.ui.cancel_holds();
        true
    }

    /// Open the platform's installed Peak View. Generation starts when the host sees this screen.
    pub fn show_peak_view(&mut self) -> bool {
        if self.state.peak_view_profile.is_none() {
            return false;
        }
        let screen = screen::PeakViewScreen::new(self.fresh_position());
        screen::apply(&mut self.ui.stack, screen::Transition::Push(Screen::PeakView(screen)));
        self.ui.map_dirty = true;
        true
    }

    pub fn set_peak_view_status(&mut self, status: crate::peak_view::runtime::Status) {
        if let Some(Screen::PeakView(screen)) = self.ui.stack.last_mut() {
            if screen.set_status(status) {
                self.ui.map_dirty = true;
            }
        }
    }

    /// A recent receiver fix, never an undated coordinate restored from storage.
    pub fn fresh_position(&self) -> Option<Fix> {
        self.tick_state
            .last_fix_ms
            .filter(|at| self.ui.now_ms.wrapping_sub(*at) <= POSITION_FIX_FRESH_MS)
            .and(self.state.user_fix)
    }

    /// Held until the open view receives a current position, including under a drawer.
    pub fn peak_view_needs_position(&self) -> bool {
        self.peak_view_base().is_some_and(|screen| screen.needs_position())
    }

    pub fn peak_view_heading_q4(&self) -> u16 {
        match self.peak_view_base() {
            Some(screen) => screen.heading_q4(&self.state),
            None => self.state.peak_view_profile.map(|profile| profile.default_heading_q4).unwrap_or(0),
        }
    }

    pub fn peak_view_position(&self) -> Option<(i32, i32)> {
        self.peak_view_base().and_then(|screen| screen.browse_position)
    }

    pub fn peak_view_is_base(&self) -> bool {
        self.peak_view_base().is_some()
    }

    /// Text and credits can cover the panorama; photos need its shared scratch arena.
    pub fn peak_view_retains_panorama(&self) -> bool {
        matches!(
            self.ui.stack.iter().rev().find(|screen| {
                !screen.is_overlay() && !matches!(screen, Screen::PeakArticle(_) | Screen::LandmarkSources(_))
            }),
            Some(Screen::PeakView(_))
        )
    }

    fn peak_view_base(&self) -> Option<&screen::PeakViewScreen> {
        match screen::base_screen(&self.ui.stack) {
            Some(Screen::PeakView(screen)) => Some(screen),
            _ => None,
        }
    }

    pub fn top_screen(&self) -> &Screen {
        self.ui.stack.last().expect("the stack always has the Home root")
    }

    /// Apply one device-wide [`Chord`]: a drawer toggle or direct Assistant entry. Returns
    /// whether it moved anything.
    ///
    /// It is resolved here, not in a screen, because the recogniser already swallowed the
    /// chord's constituents and the sheet must open over whatever the rider is on. Two rules
    /// live here only: a modal that declares [`Caps::blocks_chords`](crate::screen::Caps) stops
    /// every chord, and one drawer is open at a time, so the same chord again closes it.
    pub fn apply_chord(&mut self, chord: Chord) -> bool {
        if self.ui.stack.last().is_some_and(|s| s.caps().blocks_chords) {
            return false;
        }
        // The powering-off frame refuses a squeeze. It cannot be said in `Caps`, because the
        // frame is a page of the quick drawer and a drawer must never declare `blocks_chords`.
        // A sheet over it would cancel a shutdown the rider already confirmed.
        if self.power_off_requested() {
            return false;
        }
        match chord {
            Chord::Quick => self.toggle_drawer(Screen::QuickDrawer(QuickDrawerScreen::opening())),
            // The Assistant is a place of its own, not a page over the descent the squeeze came
            // from: it lands on the root pair, like the drawer's settings row, so the depth it
            // opens at does not depend on where the rider was and Back leaves it for the riding
            // view. A dropped screen releases nothing itself, so what it held is reconciled from
            // the stack shape: the corridor here, a detour below, and the find and visit state on
            // the next frame's `prepare_find`.
            Chord::Assistant => {
                let assistant = Screen::Assistant(screen::AssistantScreen::new());
                screen::apply(&mut self.ui.stack, screen::Transition::OverRoot(assistant));
                self.ui.map_dirty = true;
                self.ui.last_input_ms = self.ui.now_ms;
                self.ui.idle_return_timing = true;
                self.ui.cancel_holds();
                self.ui.reconcile_corridor(self.up_ahead_scope());
                self.release_unreachable_plans();
                true
            }
            // A base screen that declares no `ContextMenu` gets nothing, not an empty drawer.
            // The squeeze is still swallowed by the recogniser, so it leaks no step or Back.
            Chord::Context => match self.base_context() {
                Some(menu) => {
                    let lang = self.settings().language;
                    self.toggle_drawer(Screen::ContextDrawer(ContextDrawerScreen::opening(menu, lang)))
                }
                None => false,
            },
        }
    }

    /// What the Up-ahead timeline is scoped to right now: the live category filter and the
    /// persisted source preference. One value, so the two halves cannot reach the runtime apart.
    pub(crate) fn up_ahead_scope(&self) -> crate::corridor::UpAheadScope {
        crate::corridor::UpAheadScope { filter: self.state.up_ahead_filter, source: self.settings.up_ahead_source }
    }

    /// The [`ContextMenu`](crate::screen::ContextMenu) the base screen declares. It reads the
    /// lowest non-overlay row, so a sheet already up does not hide the content the chord asks
    /// about; that is what makes the same chord close the context drawer again.
    fn base_context(&self) -> Option<&'static crate::screen::ContextMenu> {
        screen::base_screen(&self.ui.stack).and_then(|s| {
            if matches!(s, Screen::Assistant(_)) && self.current_visit_index().is_some() {
                Some(&crate::screen::context_drawer::ASSISTANT_VISIT)
            } else if matches!(s, Screen::Assistant(_))
                && self.assistant_review_status() == crate::navigator::ReviewStatus::ResumeAvailable
            {
                Some(&crate::screen::context_drawer::ASSISTANT_RESUME)
            } else if matches!(s, Screen::WhatsNext(_)) && self.ui.ahead.page != crate::whats_next::Page::Timeline {
                None
            } else {
                s.context()
            }
        })
    }

    /// Put `drawer` on the stack, taking off whatever drawer was already there. A repeat of the
    /// same drawer therefore toggles it shut, and the other one swaps in rather than stacking.
    ///
    /// A sheet needs a slot of its own. At the ceiling the squeeze is refused, rather than pushed
    /// into a full stack where the arrival would be dropped without a sound. The Assistant chord
    /// beside it needs no such guard: it drops to the root pair, which always has room.
    fn toggle_drawer(&mut self, drawer: Screen) -> bool {
        let opening = drawer.row();
        // With a sheet already up its slot is reused, so only a full stack under no sheet refuses.
        if self.ui.stack.len() == self.ui.stack.capacity() && !self.ui.stack.last().is_some_and(|top| top.is_overlay())
        {
            return false;
        }
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
            // The frozen base under the other drawer was never redrawn, so the incoming sheet
            // owes the draw that takes those rows off. Covering is cheap, uncovering is not.
            if closed.is_some() {
                if let Some(top) = self.ui.stack.last_mut() {
                    top.owe_base_draw();
                }
            }
        }
        // The stack moved either way, so any hold charging underneath was aimed at a screen the
        // sheet just covered or uncovered. A chord is user activity, so the idle clock resets.
        self.ui.map_dirty = true;
        self.ui.last_input_ms = self.ui.now_ms;
        self.ui.idle_return_timing = true;
        self.ui.cancel_holds();
        true
    }

    /// The brightness the panel should be driven at this frame: the quick drawer's staged
    /// preview while its editor is on top, and the committed
    /// [`Settings::brightness`](crate::Settings) row everywhere else.
    ///
    /// The answer is derived, not latched, so "Back cancels the preview" needs no undo path: the
    /// editor closes and the next frame reads the committed row again. Only the top screen is
    /// asked, because a host-pushed modal is pushed above the drawer, and scanning the whole
    /// stack would hold an uncommitted preview behind a card the rider cannot dismiss.
    pub fn backlight_level(&self) -> u8 {
        match self.ui.stack.last() {
            Some(Screen::QuickDrawer(d)) => d.staged_brightness(),
            Some(Screen::ContextDrawer(d)) => d.staged_brightness(),
            _ => None,
        }
        .unwrap_or(self.settings.brightness)
        .min(crate::screen::BRIGHTNESS_MAX)
    }

    /// Whether the rider completed the quick drawer's guarded power-off hold. The host renders the
    /// powering-off frame this reports on, presents it, and then calls the
    /// [`PowerOff`](obc_ports::PowerOff) port — which does not return.
    pub fn power_off_requested(&self) -> bool {
        screen::powering_off(&self.ui.stack)
    }

    /// Number of POIs in the current snapshot, 0 when none has been taken.
    pub fn poi_snapshot_len(&self) -> usize {
        self.ui.poi_scratch.len()
    }

    /// Ask for a route-corridor POI snapshot: the map POIs of `filter` inside the corridor of
    /// the route ahead of `anchor_m`, frozen once taken. The query runs on the next rendered
    /// frame that carries both a map `Reader` and the streamed route. Re-arming an unchanged
    /// `(filter, anchor_m)` is a no-op; a changed key drops the stale rows and re-queries.
    ///
    /// The request belongs to the screen stack, not to this call: a screen declares the key it
    /// wants and [`reconcile_corridor`](crate::ui_runtime::UiRuntime::reconcile_corridor)
    /// re-points the scratch after every gesture and sweep, which is what disarms a request
    /// whose screen went away. A request armed here survives only until the next reconcile, so
    /// this is the test and introspection door.
    pub fn arm_corridor(&mut self, filter: obc_reader::PoiCategorySet, anchor_m: u32) {
        self.ui.corridor_scratch.arm(crate::corridor::CorridorKey {
            hours_filter: obc_reader::reader::places::HoursFilter::HideClosed,
            filter,
            anchor_m,
        });
    }

    /// Drop the held corridor snapshot and the request. The reader-build seam goes quiet again.
    pub fn clear_corridor(&mut self) {
        self.ui.corridor_scratch.disarm();
    }

    /// Drop the held corridor snapshot but keep the request armed, so the next frame with a
    /// `Reader` re-runs the identical query. This is the "re-enter refreshes" half of the
    /// frozen-snapshot contract.
    pub fn invalidate_corridor(&mut self) {
        self.ui.corridor_scratch.invalidate();
    }

    /// The frozen corridor snapshot, ascending by along-route distance. Empty until one has been
    /// taken, and for a genuinely empty corridor.
    pub fn corridor_snapshot(&self) -> &[obc_reader::CorridorPoi] {
        self.ui.corridor_scratch.entries()
    }

    /// Number of entries in the current corridor snapshot (0 when none has been taken).
    pub fn corridor_snapshot_len(&self) -> usize {
        self.ui.corridor_scratch.len()
    }

    /// Whether a corridor snapshot is armed but not yet taken, the fact
    /// [`base_needs_reader`](App::base_needs_reader) folds in.
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

    /// Seed the live settings from the host's persistent store at boot. The host calls this once
    /// after construction. It leaves the dirty flag clear, so seeding the boot value never
    /// triggers a needless write-back.
    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
        // Stamp the wall clock to the persisted local set-point so it resumes from the stored
        // time. The seed is display-only until a real source stamps the clock this boot.
        self.wall_clock.set(self.settings.local_clock(), self.ui.now_ms);
        // The value came from the store, so it is already persisted: reset the handshake to
        // Clean. A pending edit is discarded, because seeding is a boot operation, not an edit.
        self.settings_ops.note_seeded();
    }

    /// Merge the BLE-owned fields (units and device name) of a phone Config write into the live
    /// settings, preserving any pending device-edit persistence. The BLE plane already persisted
    /// the phone's write, so this only reconciles the live RAM copy and deliberately does not
    /// touch the revision handshake: a pending device edit still fires and writes the merged
    /// blob, and with nothing pending the live copy already matches the store. Only
    /// [`adopt_ble_fields`](crate::settings::Settings::adopt_ble_fields)'s narrow set crosses,
    /// so a device-only edit is never clobbered.
    pub fn merge_ble_settings(&mut self, other: &Settings) {
        self.settings.adopt_ble_fields(other);
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The live wall-clock time right now.
    pub fn wall_clock_now(&self) -> DateTime {
        self.wall_clock.now(self.ui.now_ms)
    }

    /// Authoritative local time for shared place eligibility. Persisted boot time is insufficient.
    pub fn place_local_time(&self) -> Option<(u8, u16)> {
        if !self.clock_trusted() || !self.settings.local_offset_known {
            return None;
        }
        let now = self.wall_clock_now();
        Some((
            obc_reader::weekday_from_ymd(now.year, now.month, now.day),
            u16::from(now.hour) * 60 + u16::from(now.minute),
        ))
    }

    pub fn clock_is_set(&self) -> bool {
        self.wall_clock.is_established()
    }

    /// Whether GPS or BLE established the wall clock during this boot.
    pub fn clock_trusted(&self) -> bool {
        self.clock_trust != ClockTrust::Untrusted
    }

    /// The single entry point that establishes a trusted wall clock from a real time source: GPS
    /// through `tick` and BLE through [`stamp_clock_ble`](App::stamp_clock_ble). Both funnel
    /// here so one place owns the invariant. It back-dates the epoch by `second`, the fix's
    /// seconds-into-the-minute, so the displayed minute rolls at the true instant.
    pub fn stamp_clock(&mut self, utc: DateTime, second: u8, offset: Option<i16>, source: ClockTrust) {
        // Persist the set-point only when it is worth an RRAM write. The persisted `clock` only
        // seeds the boot display clock, so a mid-ride re-stamp buys nothing a rider sees and
        // costs a store write on every minute roll. Arm a save on the first trusted stamp of the
        // boot, or when the persisted UTC offset moves. The offset is applied here, before the
        // change-check below: setting it in the BLE handler first would hide the change and drop
        // the save. The live `WallClock` re-stamps on every stamp either way.
        let first_trusted_this_boot = self.clock_trust == ClockTrust::Untrusted;
        let offset_before = (self.settings.utc_offset_min, self.settings.local_offset_known);
        if let Some(offset) = offset {
            self.settings.utc_offset_min = offset;
            self.settings.local_offset_known = true;
        }
        self.settings.clock = utc;
        let epoch = self.ui.now_ms.wrapping_sub(second as u32 * 1000);
        self.wall_clock.set(self.settings.local_clock(), epoch);
        if first_trusted_this_boot || (self.settings.utc_offset_min, self.settings.local_offset_known) != offset_before
        {
            self.settings_ops.note_edited();
        }
        self.clock_trust = source;
    }

    pub fn stamp_clock_ble(&mut self, utc_unix: u32, offset_min: i16) {
        let utc = DateTime::from_unix(utc_unix);
        let second = (utc_unix % 60) as u8;
        self.stamp_clock(utc, second, Some(offset_min), ClockTrust::Ble);
    }

    /// The current UTC unix seconds, from the wall clock. The clock's set-point is local time
    /// (the UTC anchor shifted by the offset), so the persisted UTC offset is folded back out.
    pub fn wall_unix_now(&self) -> u32 {
        let local = self.wall_clock.unix_now(self.ui.now_ms);
        (local as i64 - self.settings.utc_offset_min as i64 * 60) as u32
    }

    /// The footer's wall-clock anchor for this pass: what the device knows about the time of
    /// day. It is not a ride accumulator, so it is given to Recorder rather than derived by it.
    pub(crate) fn footer_clock(&self) -> crate::recorder::FooterClock {
        crate::recorder::FooterClock {
            unix_at_anchor: self.wall_unix_now(),
            anchor_ms: self.ui.now_ms,
            trusted: self.clock_trusted(),
        }
    }

    /// Whether this device can record a ride at all. A host reads it for the same reason a
    /// screen does: to not ask for something the device cannot do.
    pub fn can_record(&self) -> bool {
        self.pass.capabilities.recorder.record
    }

    /// Whether a ride is open: recording, paused, or closing.
    pub fn recording(&self) -> bool {
        self.recorder.recording()
    }

    /// The open ride's session id, or `None`. An executor keys its ride log on it; a change
    /// means open a new log.
    pub fn ride_session(&self) -> Option<u32> {
        self.recorder.session()
    }

    /// Test hook: arm a pending settings save without driving a real edit.
    #[cfg(test)]
    fn arm_settings_save(&mut self) {
        self.settings_ops.arm_save();
    }

    /// Whether the top screen would draw a live hold fill for its current selection. A
    /// render-on-demand host combines this with the charging hold progress to redraw only when
    /// the fill would animate; holding Select elsewhere changes no pixels.
    pub fn top_wants_hold_fill(&self) -> bool {
        self.ui.stack.last().is_some_and(|s| {
            s.wants_hold_fill(
                &self.settings,
                &self.state,
                self.navigator.route_state(),
                self.recorder.recording(),
                self.catalogs.routes(),
                self.catalogs.rides(),
            )
        })
    }

    /// Debug and benchmark hook: set the map camera to exactly `mpp` metres per pixel and force
    /// one redraw, so a render sweep can pin an exact scale per sample.
    pub fn set_map_mpp(&mut self, mpp: f32) {
        self.state.zoom = zoom_for_mpp(mpp);
        self.ui.map_dirty = true;
    }

    /// Recognise this frame's raw control input, apply each resulting gesture to the top screen,
    /// then advance the visible screens' timed content. It fuses the two planes into one call
    /// for single-loop hosts. Call it once per frame even with no pending events: that is how a
    /// held button's long-press fires.
    ///
    /// The two-plane firmware does not call this. Its high-priority plane recognises gestures
    /// and feeds them back through [`apply_gesture`](App::apply_gesture), while
    /// [`advance_animations`](App::advance_animations) runs on the map plane.
    pub fn handle_input(&mut self, clock: InputClock, input: &mut dyn InputSource) {
        self.ui.now_ms = clock.0;
        // The borrow split is the point: `recognize` borrows `self.ui.input`, so gestures are
        // buffered and applied after it returns. Recognition reads only the raw events and clock.
        let mut pending: heapless::Vec<Gesture, GESTURE_BUF> = heapless::Vec::new();
        let chord = self.ui.input.recognize(clock, input, |g| {
            let _ = pending.push(g);
        });
        // The chord resolves first, so this frame's gestures land on whatever the drawer left
        // on top.
        if let Some(chord) = chord {
            self.apply_chord(chord);
        }
        self.apply_gesture_batch(&pending);
        // A single-loop host has no second recognizer to cancel, so it consumes the latch the batch
        // may have set rather than leaving it for a plane that does not exist.
        let _ = self.take_hold_cancel();
        self.advance_animations(clock);
    }

    /// Recognise this frame's raw input into gestures without applying them: the recognition
    /// half of [`handle_input`](App::handle_input), for a single-loop host that drives
    /// [`run_pass`](App::run_pass) and hands the batch in as
    /// [`PassInputs::gestures`](crate::device_core::PassInputs).
    ///
    /// The clock is the recogniser's alone. The map plane's `now_ms` is the pass's to set at its
    /// input stage, because `run_pass` brackets its render-key comparison around every
    /// clock-driven change and moving `now_ms` early would hide one. So a chord resolved here is
    /// applied against the previous pass's clock, and a drawer is not handed one: its open starts
    /// on the first frame that ticks it.
    pub fn recognize(&mut self, clock: InputClock, input: &mut dyn InputSource) -> heapless::Vec<Gesture, GESTURE_BUF> {
        let mut pending: heapless::Vec<Gesture, GESTURE_BUF> = heapless::Vec::new();
        let chord = self.ui.input.recognize(clock, input, |g| {
            let _ = pending.push(g);
        });
        // A chord is not a gesture and never reaches the pass's gesture batch: it is resolved
        // here, above the screen stack.
        if let Some(chord) = chord {
            self.apply_chord(chord);
        }
        pending
    }

    /// Apply one frame's recognised gestures in order, dropping a `Hold` or `BackHold` that was
    /// already recognised into the batch behind a gesture that changed the screen stack.
    ///
    /// The transition cancels any hold still charging on the recogniser, but a completed hold
    /// sitting in this batch escaped it: it was aimed at the old top, and completing it onto the
    /// new one can be destructive. The board applies the same rule around its own gesture
    /// channel, because it must also cancel its second input plane.
    pub(crate) fn apply_gesture_batch(&mut self, gestures: &[Gesture]) {
        let mut cancelled = false;
        for &g in gestures {
            if cancelled && matches!(g, Gesture::Hold | Gesture::BackHold) {
                continue;
            }
            cancelled |= self.apply_gesture_reporting_stack_change(g);
        }
    }

    /// Drain the pending hold-cancel edge: `true` when a gesture changed the screen stack since
    /// the last drain, so any hold charging on the host's input plane is aimed at a vanished
    /// target and must be cancelled. The two-plane firmware checks this after each drained
    /// gesture; [`handle_input`](App::handle_input) consumes it itself.
    pub fn take_hold_cancel(&mut self) -> bool {
        self.ui.take_hold_cancel()
    }

    /// Apply one recognised gesture to the top screen and run the navigation transition it
    /// returns: the map plane's half of input handling. The two-plane firmware calls this per
    /// gesture from its high-priority plane, so the transition lands a frame after the overlay
    /// confirmed the press.
    pub fn apply_gesture(&mut self, g: Gesture) {
        let _ = self.apply_gesture_reporting_stack_change(g);
    }

    /// The global escape: a completed Back-hold leaves whatever the rider is on and lands on the
    /// main [`Menu`](crate::screen::MenuScreen). Reports whether the stack moved.
    ///
    /// Three rules make it up. Any sheet goes with it, because a drawer is not a place to come
    /// back to; the general [`close_drawers`](crate::screen::close_drawers) rule takes it as the
    /// Menu lands. A decision the rider must answer keeps its answer: the declared
    /// [`Caps::blocks_escape`](crate::screen::Caps) set and the terminal powering-off frame
    /// refuse the escape, while every other modal lets the Menu open over it. And it reaches the
    /// Menu without ever adding one: with a Menu already on the stack it rewinds instead of
    /// stacking a second.
    ///
    /// That last rule is a bound, not a nicety. Escape, re-descend, escape is the gesture's most
    /// ordinary use, and pushing every time would let two laps reach
    /// [`MAX_DEPTH`](crate::screen::MAX_DEPTH), where the next host-pushed card is dropped.
    ///
    /// The first escape pushes, so Back out of the Menu returns to the view the rider escaped
    /// from. Later escapes land on that same Menu, with the station they last used selected.
    fn escape_to_menu(&mut self) -> bool {
        // Asked of the base, not of `stack.last()`: a sheet opened over a card the rider must
        // answer is not consent to walk away from the card.
        let base = screen::base_screen(&self.ui.stack);
        if base.is_some_and(|s| s.caps().blocks_escape) || self.power_off_requested() {
            return false;
        }
        // No explicit sheet-popping: a rewind truncates to the Menu, which is under every
        // overlay, and a push goes through `screen::apply`, whose `Push` arm closes drawers.
        let mut changed = false;
        match self.ui.stack.iter().rposition(|s| matches!(s, Screen::Menu(_))) {
            Some(i) => {
                if i + 1 < self.ui.stack.len() {
                    self.ui.stack.truncate(i + 1);
                    changed = true;
                }
            }
            // `Root` is the fallback for a full stack with no Menu on it. Landing on
            // `[Home, Menu]` loses the way back, which beats an escape that does not escape.
            None => {
                let menu = Screen::Menu(MenuScreen::new());
                let t = if self.ui.stack.len() < self.ui.stack.capacity() {
                    screen::Transition::Push(menu)
                } else {
                    screen::Transition::Root(menu)
                };
                screen::apply(&mut self.ui.stack, t);
                changed = true;
            }
        }
        // The corridor snapshot follows the stack: escaping off the Up-ahead timeline disarms its
        // query exactly as a Back would.
        self.ui.reconcile_corridor(self.up_ahead_scope());
        if changed {
            self.ui.cancel_holds();
            self.release_unreachable_plans();
        }
        changed
    }

    /// [`apply_gesture`](App::apply_gesture), reporting whether the transition changed the screen
    /// stack. [`apply_gesture_batch`](App::apply_gesture_batch) needs that without consuming the
    /// hold-cancel latch a second input plane still owns.
    fn apply_gesture_reporting_stack_change(&mut self, g: Gesture) -> bool {
        if g == Gesture::Press && self.activate_place_detail() {
            return true;
        }
        // Every screen renders into the map plane, so an applied gesture dirties it. It is
        // conservative on purpose: with no gesture, this never runs and the map stays clean.
        self.ui.map_dirty = true;
        // Any recognised gesture is user activity, even one the screen ignores.
        self.ui.last_input_ms = self.ui.now_ms;
        self.ui.idle_return_timing = true;
        if g == Gesture::Press {
            if let Some(Screen::Assistant(screen)) = self.ui.stack.last() {
                let selected = screen.selected;
                let before = self.ui.stack.len();
                match screen::assistant::QUESTIONS[selected] {
                    Msg::AssistantFind => self.open_find_place(),
                    Msg::AssistantNext => self.open_whats_next(),
                    Msg::AssistantEasier => {
                        let result = self
                            .place_map_key()
                            .ok_or(crate::navigator::VisitUnavailable::SourceChanged)
                            .and_then(|map| self.open_easier_routes(map));
                        if let Err(error) = result {
                            if let Some(Screen::Assistant(screen)) = self.ui.stack.last_mut() {
                                screen.error = Some(error);
                            }
                        }
                    }
                    Msg::AssistantLandmarks => self.open_landmarks(),
                    _ => {}
                }
                return self.ui.stack.len() != before;
            }
        }
        if self.easier_gesture(g) {
            return true;
        }
        if g == Gesture::BackHold {
            let changed = self.escape_to_menu();

            return changed;
        }
        // Snapshot the settings so a screen edit is detected by one `==`.
        let settings_before = self.settings;
        let place_local = self.place_local_time();
        // The detour level before the screen speaks, so a cancellation takes the preview with it.
        let detour_planned_before = self.navigator.detour_planned();
        let backlight_available = self.backlight_available;
        let App { state, activity, settings, catalogs, recorder, ui, navigator, dfu, storage, metadata, .. } = self;
        let mut cx = Ctx {
            find: &mut ui.find,
            landmarks: &mut ui.landmarks,
            ahead: &mut ui.ahead,
            place_local,
            state,
            activity,
            settings,
            navigator,
            recorder,
            dfu,
            storage,

            routes: catalogs.routes(),
            rides: catalogs.rides(),
            trips: catalogs.trips(),
            trip_progress: metadata.progress(),
            day_join: metadata.day_join(),
            backlight: backlight_available,
            poi_scratch: &ui.poi_scratch,
            corridor: ui.corridor_scratch.entries(),
            sensor_scan_hits: ui.sensor_scan_hits.as_slice(),
            now_ms: ui.now_ms,
        };
        let mut t = ui.stack.last_mut().expect("the stack always has the Home root").handle(g, &mut cx);
        let depth_before = ui.stack.len();
        // Pop and Home at the root are no-ops, so they do not change the stack.
        let stack_changed = match &t {
            screen::Transition::None => false,
            screen::Transition::Pop | screen::Transition::Home => depth_before > 1,
            screen::Transition::Push(_)
            | screen::Transition::Replace(_)
            | screen::Transition::OverRoot(_)
            | screen::Transition::Root(_) => true,
        };
        if let screen::Transition::Push(Screen::PeakView(screen)) = &mut t {
            *screen = screen::PeakViewScreen::new(self.fresh_position());
        }
        screen::apply(&mut self.ui.stack, t);
        // Admit Start before the next gesture, after its requested screen transition, so a
        // recovery decision takes precedence over the requested riding view.
        self.advance_recorder_session();
        self.sync_find_preferences();
        self.handle_find_action();
        self.sync_detour_preview(detour_planned_before);
        self.release_unreachable_plans();
        // Opening a POI list drops the previous snapshot so its first draw re-queries. Gated on
        // a fresh open, so a step within the list does not wipe the frozen snapshot.
        if self.ui.stack.len() > depth_before && matches!(self.ui.stack.last(), Some(Screen::PoiList(_))) {
            self.ui.poi_scratch.invalidate();
        }
        // The corridor snapshot follows the stack, not a gesture: the Up-ahead screen on it
        // declares the key it wants and this arms it. With nothing asking, the request is
        // dropped and the reader-build seam goes quiet.
        let scope = self.up_ahead_scope();
        self.ui.reconcile_corridor(scope);
        // Returning to the bare Home root re-opens the screensaver, so re-roll its contour seed.
        // Gated on the edge, so it fires once per return and a timed re-render leaves it put.
        if self.ui.stack.len() == 1 && depth_before > 1 {
            if let Some(Screen::Home(home)) = self.ui.stack.first_mut() {
                home.reseed(self.ui.now_ms);
            }
        }
        // The top screen changed under the rider's finger, so cancel any hold charging now: a
        // long-press aimed at the old top must not complete onto the new one.
        if stack_changed {
            self.ui.cancel_holds();
        }
        if self.settings != settings_before {
            // A rider edit: bump the revision and re-arm the save, superseding an older one.
            self.settings_ops.note_edited();
            // A change to the local set-point re-stamps the wall clock. It does not touch
            // `clock_trust`: nudging the offset is not a real time source.
            let local_now = self.settings.local_clock();
            if local_now != settings_before.local_clock() {
                self.wall_clock.set(local_now, self.ui.now_ms);
            }
        }

        stack_changed
    }

    /// Advance the map plane's clock to `clock` and poll each visible screen's timers
    /// ([`Screen::tick_timers`]) in one pass. A time-driven repaint that fires dirties the map,
    /// so a screen surfaces its own timed refresh rather than the host re-rendering on a blind
    /// heartbeat, and the soonest residual deadline is stored for
    /// [`ms_until_next_wake`](App::ms_until_next_wake).
    ///
    /// [`handle_input`](App::handle_input) calls this for the single-loop hosts; the two-plane
    /// firmware calls it directly on its map plane.
    pub fn advance_animations(&mut self, clock: InputClock) {
        self.advance_easier();
        let now = self.wall_clock.now(clock.0);
        let ms_to_next_minute = self.wall_clock.ms_to_next_minute(clock.0);
        let pan_active = self.state.pan.is_some();
        let tracking = self.recorder.recording();
        // The timer poll is the UI runtime's; this method sequences the per-pass sweeps around it.
        self.ui.advance_timers(clock.0, now, ms_to_next_minute, &self.settings, pan_active, tracking);
        if matches!(self.top_screen(), Screen::Map(_)) {
            if let Some(delay) = self.ui.map_icons.wake_in(clock.0) {
                if delay == 0 {
                    self.ui.map_dirty = true;
                }
                self.ui.next_wake_ms = Some(self.ui.next_wake_ms.map_or(delay, |wake| wake.min(delay)));
            }
        }
        if self.planning_banner().is_some() {
            let remaining = 1_000 - clock.0 % 1_000;
            self.ui.next_wake_ms = Some(self.ui.next_wake_ms.map_or(remaining, |wake| wake.min(remaining)));
        }
        if let Some(remaining) = self.ui.input.chord_remaining_ms(clock.0) {
            self.ui.next_wake_ms = Some(self.ui.next_wake_ms.map_or(remaining, |wake| wake.min(remaining)));
        }
        let place_local = self.place_local_time();
        if self.ui.stack.iter().any(|screen| {
            matches!(
                screen,
                Screen::PoiList(_)
                    | Screen::PoiDetail(_)
                    | Screen::FindPlace(_)
                    | Screen::VisitReview(_)
                    | Screen::Landmarks(_)
            )
        }) && self.ui.poi_scratch.clock_changed(place_local, self.settings.utc_offset_min)
        {
            self.ui.map_dirty = true;
        }
        if self.ui.corridor_scratch.armed().is_some()
            && self.ui.corridor_scratch.clock_changed(place_local, self.settings.utc_offset_min)
        {
            self.ui.map_dirty = true;
        }
        // A kept Overview holds no query, so only its own quarter-hour stamp can ask for a new one.
        // Only a visible Overview asks: a covered one would ask on every pass.
        if matches!(self.ui.stack.last(), Some(Screen::WhatsNext(_))) && self.ui.ahead.overview_expired(place_local) {
            self.ui.map_dirty = true;
        }
        if self.ui.stack.iter().any(|screen| {
            matches!(
                screen,
                Screen::PoiList(_)
                    | Screen::PoiDetail(_)
                    | Screen::FindPlace(_)
                    | Screen::VisitReview(_)
                    | Screen::Landmarks(_)
                    | Screen::WhatsNext(_)
            )
        }) {
            let deadline = ms_to_next_minute;
            self.ui.next_wake_ms = Some(self.ui.next_wake_ms.map_or(deadline, |wake| wake.min(deadline)));
        }
        // The one host-pushed-card sweep: land anything a hold or a higher-ranked card deferred
        // earlier, and run the upload family's auto-close. It runs before the idle sweep, so an
        // unacknowledged card that lands this pass is not yanked Home by the idle return.
        self.sweep_cards();
        // The idle-return sweep (fire the return if we're past the deadline) and its residual wake,
        // folded into the deadline the event-driven host arms so a parked device wakes to return.
        self.ui.apply_idle_return(&self.settings, tracking);

        if let Some(rem) = self.ui.idle_return_remaining_ms(&self.settings, tracking) {
            self.ui.next_wake_ms = Some(self.ui.next_wake_ms.map_or(rem, |w| w.min(rem)));
        }
        // Every sweep above can move the stack, so re-point the corridor snapshot at what the
        // stack now wants: a request left armed after its screen was swept away would keep the
        // board building the map `Reader` forever.
        let scope = self.up_ahead_scope();
        let navigation = self.navigator.route_state();
        self.ui.reconcile_next_ahead(&self.settings, scope, navigation.active_route, navigation.progress_m);
    }

    /// The single next-wake deadline the event-driven host arms one timer to: the soonest, in
    /// millis from `now_ms`, that any visible screen needs a timed redraw, or `None` when
    /// nothing is time-animating. It reads the deadline
    /// [`advance_animations`](App::advance_animations) stored, so call it right after that, in
    /// the same frame and with the same `now_ms` (debug-asserted).
    pub fn ms_until_next_wake(&self, now_ms: u32) -> Option<u32> {
        debug_assert_eq!(
            now_ms, self.ui.now_ms,
            "ms_until_next_wake must follow advance_animations in the same frame, with the same now_ms"
        );
        if self.photo_pending()
            || self.landmarks_pending()
            || self.find_preparing()
            || self.assistant_route_pending()
            || (matches!(self.top_screen(), Screen::VisitReview(_))
                && self.assistant_review_status() == crate::navigator::ReviewStatus::Planning)
        {
            Some(self.ui.next_wake_ms.unwrap_or(1).min(1))
        } else {
            let icons =
                matches!(self.top_screen(), Screen::Map(_)).then(|| self.ui.map_icons.wake_in(now_ms)).flatten();
            match (self.ui.next_wake_ms, icons) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            }
        }
    }

    /// Render the current screen and any overlays above it into `target`, a `w`×`h` pixel
    /// display. It draws from the topmost opaque screen upward, so an overlay composites over the
    /// still-visible map. This is the single-target convenience for a whole frame:
    /// [`render_map`](App::render_map) then [`render_overlay`](App::render_overlay) into the same
    /// target. Hosts that keep the two on separate buffers call the halves directly.
    ///
    /// `color_fn` maps a style's RGB565 to the target's pixel colour, the one display-specific
    /// policy. `scratch` is the host-owned per-frame working memory, lent for the call and
    /// meaningless between frames. It is optional because only map-drawing screens touch it, so a
    /// pure-chrome frame passes `None`. `None` under a map-drawing base is a caller bug: the map
    /// is skipped and a `debug_assert!` fires.
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

    /// Render only the map plane: the screen stack from the topmost opaque screen upward, but
    /// without the global hold-hint chrome.
    ///
    /// This is the expensive half. A host that keeps the overlay on its own buffer renders it
    /// only when the map changed, then repaints the cheap
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
        // Untimed: `NoopClock` leaves the per-stage `*_us` fields at 0.
        self.render_scene_map_timed(scratch, target, Some(reader), route, None, w, h, color_fn, &NoopClock)
    }

    /// Timed map-plane render. `reader` is the frame's whole map source: the streamed geometry and
    /// the core-only POI and hours preparation both come from it. `None` on a chrome-only frame,
    /// which skips every map source.
    #[allow(clippy::too_many_arguments)]
    pub fn render_scene_map_timed<D, F>(
        &mut self,
        scratch: Option<&mut RenderScratch>,
        target: &mut D,
        reader: Option<&Reader>,
        route: Option<&RouteReader>,
        peak_view: Option<&crate::peak_view::Panorama>,
        w: f32,
        h: f32,
        color_fn: F,
        clock: &dyn Clock,
    ) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        self.render_scene_map_photo_timed(scratch, target, reader, route, peak_view, w, h, color_fn, clock, None)
    }

    /// Render the base, prepare bounded photo work, then compose covering screens.
    #[allow(clippy::too_many_arguments)]
    pub fn render_scene_map_photo_timed<D, F>(
        &mut self,
        scratch: Option<&mut RenderScratch>,
        target: &mut D,
        reader: Option<&Reader>,
        route: Option<&RouteReader>,
        peak_view: Option<&crate::peak_view::Panorama>,
        w: f32,
        h: f32,
        color_fn: F,
        clock: &dyn Clock,
        mut photo: Option<crate::photo::FramePhoto<'_>>,
    ) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        // The one place every host states its real frame dimensions.
        self.ui.frame_size = (w as i16, h as i16);
        self.prepare_find(reader, route);
        self.prepare_peak_article(reader);
        self.prepare_landmarks(reader);
        // Gesture handling records a route-relative pan as a distance cursor, because `Ctx` owns
        // no streamed reader. Resolve it here, before `Render` borrows state read-only.
        if let Some(route) = route {
            self.state.sync_pan_route(route);
        }
        if scratch.is_some() && self.ui.base_draws_live_map() && self.ui.render_clip.is_none() {
            self.ui.map_icons.prepare(reader, &self.state.viewport(w, h), &self.settings, self.ui.now_ms);
            // The overlay has no rider switch yet; `true` is the input a switch would drive.
            self.ui.settlements.prepare(reader, &self.state.viewport(w, h), true);
        }
        // Drain the one-shot region clip (see `set_render_clip`) — `None` on every normal frame.
        let render_clip = self.ui.render_clip.take();

        // Rebuild the cached elevation profile when the active route changes — it streams every
        // chunk, so it's built once on load, never per frame; clears when no route is loaded.
        self.navigator.refresh_route_profile(route);
        if self.ui.stack.iter().any(|s| matches!(s, Screen::WhatsNext(_))) {
            let scope = self.up_ahead_scope();
            let local = self.place_local_time();
            self.ui.ahead.prepare(reader, route, self.navigator.climbs(), scope, &mut self.ui.corridor_scratch, local);
            if self.ui.ahead.pending() {
                self.ui.map_dirty = true;
                self.ui.next_wake_ms = Some(1);
            }
        }
        // Drop the resident ride profile and track preview the moment they stop matching the
        // viewed ride. Filling is the executor's keyed answer; only the drop lives here.
        let key = self.catalogs.ride_track_key(self.activity.viewed_ride);
        self.catalogs.drop_stale_ride_views(key);

        // Pre-draw acquisition: the base screen resolves any streamed-reader state before the
        // draw loop, so every screen's `draw` is side-effect-free.
        let navigation = self.navigator.route_state();
        self.ui.prepare_base(
            reader,
            route,
            self.state.user_fix,
            navigation.active_route,
            navigation.progress_m,
            navigation.route_total_m,
            self.catalogs.detour_preview_for(navigation.active_route),
            self.place_local_time(),
        );

        // Computed before the field borrow below splits `self`.
        let now = self.wall_clock.now(self.ui.now_ms);
        let clock_set = self.wall_clock.is_established();
        let place_local = self.place_local_time();
        let base = screen::base_index(&self.ui.stack);

        // The in-screen confirm fill's hold-progress. Prefer a host-supplied value (the two-plane
        // firmware's separate input plane); fall back to `App`'s own input on the single-loop hosts.
        let hold_progress =
            self.ui.hold_progress_override.map_or_else(|| self.ui.input.select_hold_progress(), |p| p.select);
        let no_fix = !self.has_live_fix(self.ui.now_ms);
        let backlight_available = self.backlight_available;
        let visit_target = self.assistant_visit_target();

        let assistant_preview = matches!(&self.ui.stack[base], Screen::Easier(_) | Screen::VisitReview(_)).then(|| {
            if matches!(&self.ui.stack[base], Screen::VisitReview(s) if s.accepted) {
                self.current_visit_index().and_then(|i| self.route_ids().get(i).copied())
            } else {
                self.assistant_preview().map(|p| p.source.object)
            }
        });
        let App {
            state,
            activity,
            settings,
            catalogs,
            navigator,
            recorder,
            ui,
            fw_version,
            map_name,
            map_obcm_version,
            storage,
            metadata,
            ..
        } = self;
        // The shape previews draw only for the subject they were decimated for — a stale key
        // (route/ride changed, preview not re-fed yet) hands the screens an empty slice.
        let navigation = navigator.route_state();
        let preview_index = if let Some(source) = assistant_preview {
            source.and_then(|id| catalogs.route_ids().iter().position(|value| *value == id))
        } else {
            navigation.active_route
        };
        let nav_key = catalogs.nav_preview_key(preview_index, assistant_preview.is_some());
        let ride_key = catalogs.ride_track_key(activity.viewed_ride);
        let nav_preview: &[(i32, i32)] = catalogs.nav_preview_for(nav_key);
        let ride_preview: &[(i32, i32)] = catalogs.ride_preview_for(ride_key);
        let detour_preview: &[(i32, i32)] = catalogs.detour_preview_for(navigation.active_route);
        // The resident detail buffer is only meaningful while a climb is active, so hand out the
        // pair exactly when `active_climb` resolves to a live segment.
        let climb = navigation
            .active_climb
            .and_then(|i| navigator.climbs().as_slice().get(i))
            .map(|seg| screen::ActiveClimb { seg, profile: navigator.climb_profile() });
        let rx = Render {
            visit_target,
            find: &ui.find,
            landmarks: &ui.landmarks,
            map_icons: &ui.map_icons,
            settlements: &ui.settlements,
            ahead: &ui.ahead,
            peak_view,
            scratch,

            state,
            navigation,
            recorder,
            settings,
            routes: catalogs.routes(),
            unaccepted_routes: navigator.unaccepted_routes(),
            internal_routes: navigator.internal_routes(),
            rides: catalogs.rides(),
            trips: catalogs.trips(),
            trip_progress: metadata.progress(),
            day_join: metadata.day_join(),
            route,
            profile: navigator.profile(),
            ride_profile: catalogs.ride_profile_for(ride_key),
            climb,
            climbs: navigator.climbs(),
            waypoints: navigator.waypoints(),
            breadcrumb: &recorder.breadcrumb,
            recording: recorder.recording(),
            nav_preview,
            ride_preview,
            detour_preview,
            poi_scratch: &ui.poi_scratch,
            corridor: ui.corridor_scratch.entries(),
            corridor_status: ui.corridor_scratch.status(),
            next_ahead: &ui.next_ahead,
            sensor_status: ui.sensor_status.as_slice(),
            sensor_scan_hits: ui.sensor_scan_hits.as_slice(),
            w: w as i32,
            h: h as i32,
            now_ms: ui.now_ms,
            marquee: ui.marquee.frame(),
            now,
            clock_set,
            place_local,
            hold_progress,
            no_fix,
            clock,
            stats: RenderStats::default(),
            fw_version: fw_version.as_str(),
            map_name: map_name.as_str(),
            map_obcm_version: *map_obcm_version,
            card_free_bytes: storage.free_bytes(),

            backlight: backlight_available,
        };
        let mut rx = RenderFrame { scene: reader, render: rx };
        // A drawer recesses the base rather than replacing it: the base draws through the dim
        // LUT composed with the host's colour policy, the sheet through the untouched one. No
        // capture buffer, and no alpha for a 64-colour panel to approximate. Whether it recesses
        // at all is the base's own `Caps::recess` declaration, false for a map base.
        //
        // The switch is a `Cell` inside one colour closure, not a second `Canvas` with a second
        // closure type: `Screen::draw` is generic over the colour function, so a second closure
        // type monomorphises the whole screen catalogue and the map renderer again, about
        // +147 KB of flash. `Canvas` resolves `color_fn` once per primitive, so the branch costs
        // O(primitives), not O(pixels).
        let covered = ui.base_frozen();
        let recessed = covered && ui.stack.get(base).is_none_or(|s| s.caps().recess);
        let recess = core::cell::Cell::new(recessed);
        let policy = |c: u16| color_fn(if recess.get() { screen::dim_color(c) } else { c });
        // A drawer's render key already shadows the base's, so no fact the base draws can move
        // while a sheet is up: its rows on the panel are still right. On a resident target the
        // base's draw is therefore skipped, and an open step costs the sheet and nothing else.
        // The three exclusions are on [`UiRuntime::sheet_only`].
        let preserve_photo = photo.as_ref().is_some_and(|work| !work.redraw)
            && ui.resident_frame
            && matches!(ui.stack.get(base), Some(Screen::LandmarkPhoto(_)));
        let sheet_only = ui.sheet_only() || preserve_photo;
        // The one Canvas of the frame: every screen draws through it, and only the base screen
        // writes `rx.stats`. A drained region clip makes it reject whole out-of-region
        // primitives, which the target's pixel clip cannot do.
        let mut cv = Canvas::new(target, &policy);
        cv.set_clip(render_clip);
        for i in base..ui.stack.len() {
            if !(i == base && sheet_only) {
                if let Screen::LandmarkPhoto(page) = &mut ui.stack[i] {
                    page.invalidate(covered);
                }
                ui.stack[i].draw(&mut cv, &mut rx);
            }
            if i == base {
                if let (Some(work), Screen::LandmarkPhoto(page)) = (photo.as_mut(), &mut ui.stack[i]) {
                    if !covered || page.covered_rebuild {
                        let (target, color) = cv.split();
                        for _ in 0..work.steps {
                            work.runtime.step(page, reader, target, color, rx.settings.language);
                            if !matches!(page.status, crate::photo::Status::Fresh | crate::photo::Status::Pending) {
                                page.covered_rebuild = false;
                                break;
                            }
                        }
                    } else {
                        work.runtime.cancel();
                    }
                }
                recess.set(false);
            }
        }
        // Read out before the debt is discharged, so `rx`'s borrow of `ui` ends first.
        let stats = rx.stats;
        let marquee = rx.marquee.request();
        // A fresh name's first step is armed here, after the draw that named it. The pass's own
        // wake was planned before the render, so the firmware re-reads the deadline after it.
        if let Some(wake) = ui.marquee.adopt(marquee, ui.now_ms) {
            ui.next_wake_ms = Some(ui.next_wake_ms.map_or(wake, |w| w.min(wake)));
        }
        // The frame pays what the sheets above the base owed. A sheet arms a base draw when it
        // stops purely covering the screen below, and carries it until a frame draws that
        // screen. This is that frame; a pass may tick and render nothing at all.
        if !sheet_only {
            ui.spend_base_draw();
        }
        stats
    }

    /// Render only the overlay plane: the transient always-on-top chrome, over whatever is
    /// already in `target`.
    ///
    /// Compositing contract, so this can live on its own buffer: it paints only its own pixels
    /// and never clears the rest of the target. It must be valid drawn over arbitrary content, so
    /// a host can repaint it over an unchanged map. Poll
    /// [`overlay_active`](App::overlay_active) to decide whether a repaint is needed.
    pub fn render_overlay<D, F>(&self, target: &mut D, w: f32, h: f32, color_fn: F)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        self.ui.input.render_overlay(target, w, h, &color_fn);
        self.render_planning_banner(target, w, h, color_fn);
    }

    /// Paint the request's busy label after a base redraw, including gaps between planner runs.
    pub fn render_planning_banner<D, F>(&self, target: &mut D, w: f32, h: f32, color_fn: F)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        if let Some(message) = self.planning_banner() {
            let text = crate::i18n::t(message, self.settings.language);
            crate::screen::vocab::chrome::recalculating_banner(
                target,
                &color_fn,
                w,
                h,
                text,
                self.planning_banner_phase(),
            );
        }
    }

    /// The planning banner's bounding rows `[y0, y0 + rows)` in a `w`×`h` frame, or `None` when
    /// the freeze is not engaged. A partial-overlay host pushes the union of this and the hold
    /// bulge's rows to present exactly what changed.
    pub fn reroute_banner_rows(&self, h: f32) -> Option<(u16, u16)> {
        self.planning_banner().map(|_| crate::screen::vocab::chrome::recalculating_banner_rows(h))
    }

    /// Whether the overlay plane has live content this frame. `false` exactly when
    /// [`render_overlay`](App::render_overlay) would draw nothing, so a host driving the overlay
    /// as a separate layer can leave it idle.
    pub fn overlay_active(&self) -> bool {
        self.ui.input.overlay_active() || self.planning_banner().is_some()
    }

    /// Drain the repaint demand accumulated since the last call, resetting to [`Dirty::CLEAN`].
    /// The host calls this once per frame, then renders each plane only when its flag is set.
    ///
    /// [`map`](Dirty::map) accumulates every map-affecting mutation since the last drain.
    /// [`overlay`](Dirty::overlay) is derived from the hold bulge's and the planning freeze's
    /// levels, plus one trailing frame after the bulge goes quiet so the host can clear it. That
    /// trailing edge is tracked across calls, so draining twice in one frame swallows it.
    ///
    /// [`region`](Dirty::region) carries the accumulated region-scoped demand, but only when no
    /// full-frame demand joined it since the last drain: a set `map_dirty` covers any region.
    pub fn take_dirty(&mut self) -> Dirty {
        let overlay = crate::device_core::pass::OverlayKey {
            hold: self.ui.input.overlay_active(),
            banner: self.planning_banner_phase(),
        };
        let overlay = self.pass.overlay_repaint(overlay);
        let mut dirty = self.ui.take_dirty();
        dirty.overlay = overlay;
        dirty
    }

    /// The most recently recognised gesture. No production host reads it; the two-plane input
    /// tests do, to prove the map plane's own recogniser stays dormant.
    pub fn last_gesture(&self) -> Option<Gesture> {
        self.ui.input.last_gesture()
    }

    /// Feed the live hold progress (0.0 to 1.0) of both hold buttons. The two-plane firmware calls
    /// this each frame from its high-priority [`InputPlane`], whose hold state `App`'s own plane
    /// does not see; without it the Reset bar never fills and no hold ever defers a card. The
    /// single-loop hosts never call it.
    pub fn set_hold_progress(&mut self, select: f32, back: f32) {
        self.ui.hold_progress_override = Some(crate::ui_runtime::HoldSample { select, back });
    }

    /// Arm the one-shot region clip for the next [`render_scene_map_photo_timed`](App::render_scene_map_photo_timed).
    /// The host that drained a [`Dirty`](crate::Dirty) whose [`region`](crate::Dirty::region)
    /// survived calls this right before rendering, and the frame's `Canvas` then skips whole
    /// primitives whose bounds miss it. Pair it with a matching pixel clip on the framebuffer:
    /// rejection alone leaves straddling primitives painting outside the region. The render
    /// clears it.
    pub fn set_render_clip(&mut self, clip: Option<Rectangle>) {
        self.ui.render_clip = clip;
    }

    /// Declare that this host's render target keeps the last frame between renders: a resident
    /// panel plane, as against a buffer composed from nothing every time.
    ///
    /// A resident target is what makes a repaint able to be partial: with a drawer over the
    /// base, the base's draw is skipped and its rows simply stand. A host that says nothing gets
    /// every screen drawn every time, which is always correct. Declare it once at composition.
    ///
    /// A host that claims this untruthfully is not caught by anything: the differential replay's
    /// reference does not declare it, so a false claim simply silences the oracle. The target
    /// must be one buffer that survives between renders and is never cleared.
    pub fn set_resident_frame(&mut self, resident: bool) {
        self.ui.resident_frame = resident;
    }

    pub fn mode(&self) -> Mode {
        self.activity.mode
    }
}

impl App {
    /// Whether the "Installing update" card is on the stack: the frame an arming executor
    /// freezes onto the panel for the whole SD-to-flash stream and the warm reset that never
    /// paints. The card scheduler can bounce the install-began answer when the stack is full, so
    /// a board that armed anyway would have handed the panel a frame showing something else.
    pub fn dfu_installing_card_up(&self) -> bool {
        self.ui.stack.iter().any(|s| matches!(s, Screen::DfuInstalling(_)))
    }

    /// What DeviceCore needs read right now: a level, recomputed from state, never stored.
    ///
    /// A need stays up until an input carrying exactly its key is accepted, and a failure is
    /// such an input, so a dead file costs one read rather than one per pass. The key names a
    /// durable identity, the source revision, and the view generation, so a subject change, a
    /// re-commit or an explicit invalidate all produce a different key.
    pub fn derived_needs(&self) -> crate::device_core::derived::DerivedNeeds {
        use crate::device_core::derived::DerivedNeeds;
        let ride_track = self
            .catalogs
            .ride_track_key(self.activity.viewed_ride)
            .filter(|&key| !self.catalogs.ride_track_answered(key));
        // The screen half of the preview level is the UI's; the data half is the key's.
        let assistant = screen::base_screen(&self.ui.stack)
            .is_some_and(|s| matches!(s, Screen::VisitReview(s) if s.accepted && self.current_visit_index().is_some()));
        let overview_open = assistant || self.ui.stack.iter().any(|s| matches!(s, Screen::RouteOverview(_)));
        let nav_preview = overview_open
            .then(|| self.catalogs.nav_preview_key(self.active_route_index(), assistant))
            .flatten()
            .filter(|&key| !self.catalogs.nav_preview_answered(key));
        DerivedNeeds { ride_track, nav_preview }
    }

    /// Accept keyed derived inputs. An input whose key is not the one the need currently carries
    /// is stale: it changes nothing, and the need stays up.
    ///
    /// One ride-track answer publishes both of that need's targets from the one key: the profile
    /// the executor wrote in place, and the track shape it hands in through `targets`. Sharing a
    /// key is what stops them diverging.
    ///
    /// Refused while a DeviceCore pass runs: a platform callback must not change DeviceCore
    /// mid-pass, or a later stage would decide from a picture the earlier ones never saw.
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
        // The need's key, not the subject's: a nav preview is wanted only while an overview is
        // open, so keying on the active route alone would accept an answer that lands after the
        // rider closed it and mark the level answered.
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
    use crate::harness::support::ride_summary;
    use crate::settings::SETTINGS_RETRY_BACKOFF_MS;
    use obc_ports::{CompassSource, LocationSource};

    /// A location source that yields one fix then runs dry.
    struct OneFix(Option<Fix>);
    impl LocationSource for OneFix {
        fn poll(&mut self) -> Option<Fix> {
            self.0.take()
        }
    }

    /// A card pushed outside the input path moves the stack under whatever is charging, so it must
    /// ring the recogniser exactly as a gesture-driven move does. Setting the host's edge alone
    /// leaves the App's own hold to complete onto the card.
    #[test]
    fn the_route_cleanup_card_cancels_a_hold_charging_under_it() {
        use crate::harness::support::{down, keys};
        use obc_ports::Button;

        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.handle_input(InputClock(0), &mut keys(&[down(Button::Select)]));
        app.handle_input(InputClock(300), &mut keys(&[]));
        assert!(app.ui.input.select_hold_progress() > 0.0, "a Select hold is charging");

        app.offer_route_cleanup(crate::device_core::StoreIdentity::new(1));
        assert!(matches!(app.top_screen(), Screen::RouteCleanup(_)), "the card is up");
        assert!(app.take_hold_cancel(), "the host's own plane is told to cancel");

        // Past the 500 ms threshold: the hold was aimed at the screen the card replaced, so it must
        // never fire at all.
        app.handle_input(InputClock(700), &mut keys(&[]));
        assert!(app.ui.input.last_gesture().is_none(), "the cancelled hold does not complete onto the card");
    }

    /// The ride whose track the open detail still needs, read as the durable id.
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

    /// The revision a settings write is owed for, if one is. The operation token is dropped, so
    /// this is for a test that only asks.
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
            let effect = app.settings_ops.next_effect(in_subtree, now_ms)?;
            let crate::settings::SettingsEffect::PersistRevision { token, revision } = effect;
            self.token = Some(token);
            Some(revision)
        }

        fn ack(&mut self, app: &mut App, revision: u16) {
            let token = self.token.take().expect("a write is in flight to answer");
            let now_ms = app.ui.now_ms;
            let _ =
                app.settings_ops.apply_outcome(crate::settings::SettingsOutcome::Persisted { token, revision }, now_ms);
        }

        /// The write failed: the app keeps the revision dirty, re-arms the backoff, and warns.
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

    /// Whether leaving the settings subtree emitted a persist this pass.
    fn settings_dirty(app: &mut App) -> bool {
        drain_persist(app).is_some()
    }

    fn home_seed(app: &App) -> u32 {
        match app.ui.stack.first() {
            Some(Screen::Home(h)) => h.backdrop_seed(),
            _ => panic!("Home is always the stack root"),
        }
    }

    #[test]
    fn returning_to_home_rerolls_the_backdrop_seed() {
        let mut app = App::new_idle(AppState::new(0, 0, 0.05)); // [Home], the canonical seed
        assert_eq!(home_seed(&app), 0, "boot starts on the un-jittered massif");

        app.ui.now_ms = 4242;
        app.apply_gesture(Gesture::BackHold); // Home → Menu (stack grows)
        assert_eq!(home_seed(&app), 0, "going deeper than Home does not reseed");

        app.apply_gesture(Gesture::Back); // Menu → Pop → back to [Home]
        assert_eq!(home_seed(&app), 4242, "returning to Home re-rolls from the wall clock");

        app.ui.now_ms = 9999;
        app.apply_gesture(Gesture::Step(1));
        assert_eq!(home_seed(&app), 4242, "a no-op gesture on Home keeps the same pattern");
    }

    struct ConstCompass(f32);
    impl CompassSource for ConstCompass {
        fn poll(&mut self) -> Option<f32> {
            Some(self.0)
        }
    }

    /// An altimeter that yields one altitude sample then runs dry.
    struct OneAlt(Option<f32>);
    impl obc_ports::AltimeterSource for OneAlt {
        fn poll(&mut self) -> Option<f32> {
            self.0.take()
        }
    }

    /// A clock source that yields one GPS UTC time then runs dry.
    struct OneClock(Option<obc_ports::GpsTime>);
    impl obc_ports::ClockSource for OneClock {
        fn poll(&mut self) -> Option<obc_ports::GpsTime> {
            self.0.take()
        }
    }

    fn moving(course: f32) -> Fix {
        Fix { lat: 0, lon: 0, course: Some(course), speed_mps: Some(5.0) }
    }

    /// Tick once with only a GPS clock source, at the map-plane clock the stamp and read share.
    fn tick_clock(app: &mut App, t: obc_ports::GpsTime, now_ms: u32) {
        app.ui.now_ms = now_ms; // mirror `advance_animations(now)` running right before `tick(now)`
        let mut loc = OneFix(None);
        let mut clock = OneClock(Some(t));
        app.tick(RideClock(now_ms), Sensors { clock: Some(&mut clock), ..Sensors::new(&mut loc) }, None);
    }

    fn gps_time(hour: u8, minute: u8, second: u8) -> obc_ports::GpsTime {
        obc_ports::GpsTime { utc: DateTime { year: 2026, month: 6, day: 30, hour, minute }, second }
    }

    /// `clock_is_set` can be true from the seeded set-point while `clock_trusted` is not.
    #[test]
    fn boot_clock_is_untrusted() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        assert!(!app.clock_trusted(), "no source stamped the clock yet — untrusted from boot");
        app.set_settings(Settings {
            clock: DateTime { year: 2026, month: 6, day: 30, hour: 8, minute: 0 },
            ..Settings::default()
        });
        assert!(app.clock_is_set(), "the seeded set-point is established (the Home date line shows)");
        assert!(!app.clock_trusted(), "but a stale persisted seed is never trusted");
    }

    #[test]
    fn service_local_time_requires_clock_and_explicit_offset_authority() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        assert_eq!(app.place_local_time(), None);
        tick_clock(&mut app, gps_time(14, 37, 0), 1000);
        assert!(app.clock_trusted());
        assert_eq!(app.place_local_time(), None, "GPS establishes UTC, not the local offset");
        app.stamp_clock(gps_time(14, 37, 0).utc, 0, Some(0), ClockTrust::Ble);
        assert!(app.place_local_time().is_some(), "an explicit zero offset is also authoritative");
        assert!(app.settings.local_offset_known);
    }

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

    #[test]
    fn only_the_first_trusted_stamp_of_the_boot_persists() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.set_settings(Settings::default());
        tick_clock(&mut app, gps_time(14, 37, 10), 1000);
        assert!(app.clock_trusted(), "the first stamp establishes trust");
        assert!(settings_dirty(&mut app), "the first trusted stamp of the boot persists once");
        tick_clock(&mut app, gps_time(14, 37, 42), 5000);
        assert!(!settings_dirty(&mut app), "same-minute re-stamp doesn't re-arm a save");
        tick_clock(&mut app, gps_time(14, 38, 3), 65_000);
        assert!(!settings_dirty(&mut app), "a new-minute re-stamp still doesn't re-persist once trusted");
        assert_eq!(
            (app.wall_clock_now().hour, app.wall_clock_now().minute),
            (14, 38),
            "but the live wall clock still re-stamps every fix",
        );
    }

    #[test]
    fn ble_setclock_stamps_trusts_and_persists() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.set_settings(Settings::default());
        assert!(!app.clock_trusted(), "untrusted before the first setClock");
        // 2026-07-09T12:00:30Z at +02:00, plus 30 s to exercise the seconds-into-the-minute
        // back-date.
        app.stamp_clock_ble(1_783_598_400 + 30, 120);
        let now = app.wall_clock_now();
        assert_eq!((now.hour, now.minute), (14, 0), "UTC 12:00 + 02:00 → local 14:00");
        assert_eq!(app.settings().clock, DateTime { year: 2026, month: 7, day: 9, hour: 12, minute: 0 });
        assert_eq!(app.settings().utc_offset_min, 120, "the phone's offset is persisted");
        assert_eq!(app.clock_trust, ClockTrust::Ble, "the trust source is BLE");
        assert!(settings_dirty(&mut app), "the first trusted stamp of the boot persists once");
    }

    #[test]
    fn ble_setclock_persists_a_changed_offset_on_a_same_boot_reconnect() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.set_settings(Settings::default());
        app.stamp_clock_ble(1_783_598_400, 120);
        assert!(settings_dirty(&mut app), "first trusted stamp persists (offset 120)");
        // Already trusted, so only the offset move can arm the save.
        app.stamp_clock_ble(1_783_602_000, 60);
        assert_eq!(app.settings().utc_offset_min, 60, "the new offset is adopted");
        assert!(settings_dirty(&mut app), "a same-boot offset change persists even while already trusted");
        app.stamp_clock_ble(1_783_605_600, 60);
        assert!(!settings_dirty(&mut app), "an unchanged offset on reconnect arms no save (no RRAM thrash)");
    }

    #[test]
    fn gps_time_back_dates_the_epoch_by_seconds() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.set_settings(Settings::default());
        tick_clock(&mut app, gps_time(14, 37, 56), 10_000); // stamped 56 s into the minute
        assert_eq!((app.wall_clock_now().hour, app.wall_clock_now().minute), (14, 37));
        // 4 s on, 60 s since the minute's true start, so the minute must have rolled.
        app.ui.now_ms = 14_000;
        assert_eq!((app.wall_clock_now().hour, app.wall_clock_now().minute), (14, 38), "rolls 4 s later");
    }

    #[test]
    fn peak_view_acquires_once_while_idle_and_rejects_stale_positions_on_reentry() {
        use crate::peak_view::runtime::{Failed, Lifecycle, Platform, Progress};
        #[derive(Default)]
        struct Job(usize);
        impl Platform for Job {
            fn start(&mut self, _: &mut App, _: (i32, i32)) -> bool {
                self.0 += 1;
                true
            }
            fn step(&mut self, _: &mut App) -> Result<Progress, Failed> {
                Ok(Progress { complete: true, revision: 1 })
            }
            fn cancel(&mut self) {}
        }
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.state.peak_view_profile = Some(crate::PeakViewProfile::at(0, 0, 0));
        let fix = Fix::at(7_961_000, 46_585_000);
        app.state.user_fix = Some(fix);
        app.show_peak_view();
        assert!(app.peak_view_needs_position(), "an undated cached fix cannot start terrain");
        let mut lifecycle = Lifecycle::default();
        let mut job = Job::default();
        lifecycle.update(&mut app, &mut job, 0);
        assert_eq!(job.0, 0);
        assert!(!lifecycle.busy());
        app.apply_chord(Chord::Quick);
        assert!(app.peak_view_needs_position(), "the drawer keeps its base request");
        tick_fix(&mut app, fix, 1000);
        assert!(!app.peak_view_needs_position(), "a new fix at the same coordinate fulfills the request");
        app.apply_chord(Chord::Quick);
        lifecycle.update(&mut app, &mut job, 1000);
        assert_eq!(job.0, 1);
        assert!(!app.recording());
        app.ui.now_ms = 40_000;
        assert!(!app.peak_view_needs_position(), "an open panorama does not repeatedly wake GPS");
        app.apply_gesture(Gesture::Back);
        lifecycle.reconcile(&app);
        app.show_peak_view();
        assert!(app.peak_view_needs_position(), "reopening checks the age again");
        app.apply_gesture(Gesture::Back);
        assert!(!app.peak_view_needs_position(), "leaving cancels acquisition");
        tick_fix(&mut app, fix, 41_000);
        app.show_peak_view();
        assert!(!app.peak_view_needs_position(), "a recent fix skips the waiting screen");
    }

    #[test]
    fn menu_peak_entry_applies_the_same_freshness_rule() {
        for fresh in [false, true] {
            let mut app = App::new_idle(AppState::new(0, 0, 1.0));
            app.state.peak_view_profile = Some(crate::PeakViewProfile::at(0, 0, 0));
            tick_fix(&mut app, Fix::at(0, 0), 0);
            app.ui.now_ms = if fresh { POSITION_FIX_FRESH_MS } else { POSITION_FIX_FRESH_MS + 1 };
            app.escape_to_menu();
            app.apply_gesture(Gesture::Step(3));
            app.apply_gesture(Gesture::Press);
            assert!(app.peak_view_is_base());
            assert_eq!(app.peak_view_needs_position(), !fresh);
        }
    }

    /// Tick once with a single fix, at the map-plane clock `last_fix_ms` and `has_live_fix` share.
    fn tick_fix(app: &mut App, fix: Fix, now_ms: u32) {
        app.ui.now_ms = now_ms; // mirror `advance_animations(now)` running right before `tick(now)`
        let mut loc = OneFix(Some(fix));
        app.tick(RideClock(now_ms), Sensors::new(&mut loc), None);
    }

    /// One DeviceCore pass at `now_ms` with a fix and/or a heart-rate reading on the ports,
    /// returning what it planned to repaint. It is the only composition that compares the render
    /// keys; a bare `tick` never reaches the boundary that reads them.
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

            support: crate::harness::support::EVERY_CAPABILITY,
            outcomes: &mut outcomes,
            facts: &mut facts,
            derived: DerivedInputs::NONE,
            targets: DerivedTargets::NONE,
        });
        plan.render
    }

    fn pass_fix(app: &mut App, fix: Fix, now_ms: u32) -> Dirty {
        pass_ports(app, now_ms, Some(fix), None)
    }

    fn pass_idle(app: &mut App, now_ms: u32) -> Dirty {
        pass_ports(app, now_ms, None, None)
    }

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

    #[test]
    fn a_sheet_only_frame_needs_no_reader_and_an_uncovered_map_still_does() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
        app.set_resident_frame(true);
        assert!(app.base_needs_reader(), "an uncovered Map draws the map, so it needs the reader");

        assert!(app.apply_chord(crate::input::Chord::Quick));
        assert!(!app.base_needs_reader(), "the sheet covers an undimmed map: nothing reads the card");

        assert!(app.apply_chord(crate::input::Chord::Quick), "the same chord closes it");
        assert!(app.base_needs_reader(), "…and the uncovered map needs it again");
    }

    /// The sheet that replaces the other arrives over rows the departed sheet still holds,
    /// because the frozen base was never redrawn under it, so its first frame draws the base.
    #[test]
    fn the_drawer_that_replaces_the_other_owes_one_base_draw() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
        app.set_resident_frame(true);

        assert!(app.apply_chord(crate::input::Chord::Quick));
        assert!(app.sheet_only(), "a sheet opening over a standing map draws itself alone");

        assert!(app.apply_chord(crate::input::Chord::Context));
        assert!(matches!(app.ui.stack.last(), Some(Screen::ContextDrawer(_))), "the context sheet swapped in");
        assert!(!app.sheet_only(), "the quick drawer's rows are still on the panel: this frame draws the base");
        app.ui.spend_base_draw();
        assert!(app.sheet_only(), "…and the frame that drew it ends the debt; the open goes on sheet-only");

        assert!(app.apply_chord(crate::input::Chord::Quick));
        assert!(matches!(app.ui.stack.last(), Some(Screen::QuickDrawer(_))), "and back the other way");
        assert!(!app.sheet_only(), "the context sheet's rows are on the panel now");

        assert!(app.apply_chord(crate::input::Chord::Quick), "the same chord closes it");
        assert!(app.apply_chord(crate::input::Chord::Quick), "a sheet opening over nothing but the map…");
        assert!(app.sheet_only(), "…owes it nothing");
    }

    /// The default 1 s fix interval gives the 5 s floor window.
    #[test]
    fn has_live_fix_tracks_freshness_within_the_window() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        assert!(!app.has_live_fix(0), "no fix yet → not live (acquiring)");

        tick_fix(&mut app, Fix::at(0, 0), 1_000);
        assert!(app.has_live_fix(1_000), "just got a fix → live");
        assert!(app.has_live_fix(1_000 + 5_000), "still live at the window edge");
        assert!(!app.has_live_fix(1_000 + 5_001), "past the window → lost");
    }

    #[test]
    fn no_fix_window_scales_with_the_fix_interval() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.set_settings(Settings { fix_interval_s: 30, ..Settings::default() }); // window = 30·3 = 90 s
        tick_fix(&mut app, Fix::at(0, 0), 1_000);
        assert!(app.has_live_fix(1_000 + 60_000), "a 60 s gap is within a 30 s-interval window");
        assert!(!app.has_live_fix(1_000 + 90_001), "but past 90 s the fix is lost");
    }

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

    #[test]
    fn no_fix_flip_does_not_dirty_idle_home() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0)); // [Home], Idle — not a live-data view
        pass_fix(&mut app, Fix::at(0, 0), 1_000); // flip true→false, but Home draws no banner
        assert!(!pass_idle(&mut app, 1_000 + 6_001).map, "the no-fix flip never dirties a static Home");
    }

    #[test]
    fn tracking_arms_without_a_fix_and_stages_on_first_fix() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.test_start_ride(); // a route load arms a tracking session

        let mut loc = OneFix(None);
        app.ui.now_ms = 1_000;
        app.tick(RideClock(1_000), Sensors::new(&mut loc), None);
        assert!(app.recording(), "the session is armed immediately, fix or not");
        assert!(!app.has_live_fix(1_000), "no fix yet → the banner is up");
        assert!(app.recorder.staged().is_empty(), "nothing recorded while searching");
        assert_eq!(app.recorder.moving_s(), 0.0, "moving time idles until the first fix");

        let mut loc = OneFix(Some(Fix::at(0, 0)));
        app.ui.now_ms = 2_000;
        app.tick(RideClock(2_000), Sensors::new(&mut loc), None);
        assert!(app.has_live_fix(2_000), "the fix landed → banner clears");
        assert_eq!(app.recorder.staged().len(), 1, "the first fix stages the segment anchor");
        assert!(app.recorder.staged()[0].segment_start, "…as a segment anchor");
    }

    #[test]
    fn a_ride_log_that_loses_a_sample_raises_the_recording_error_warning() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.test_start_ride();
        assert!(!app.ui.stack.iter().any(|s| matches!(s, Screen::Warning(_))), "a healthy ride shows no warning card");

        // Nothing serves the append, so the staged samples pile up. About 11 m per second, so
        // every fix is logged and none is a teleport.
        let mut lost = None;
        for step in 0..40u32 {
            let mut loc = OneFix(Some(Fix::at(0, 100 * step as i32)));
            app.ui.now_ms = step * 1_000;
            app.tick(RideClock(step * 1_000), Sensors::new(&mut loc), None);
            if lost.is_none() {
                if let Some(card) = app.ui.stack.iter().find_map(|s| match s {
                    Screen::Warning(w) => Some(w.flags()),
                    _ => None,
                }) {
                    lost = Some((step, card));
                }
            }
        }
        let (step, card) = lost.expect("a buffer nothing drains eventually loses a sample, and says so");
        assert!(card.contains(WarningFlags::REC_ERROR), "the card carries the recording-error flag");
        assert!(step > 1, "and it is raised only once the bounded buffer is genuinely full");

        app.apply_gesture(Gesture::Back);
        let mut loc = OneFix(Some(Fix::at(0, 100 * 40)));
        app.ui.now_ms = 40_000;
        app.tick(RideClock(40_000), Sensors::new(&mut loc), None);
        assert!(
            !app.ui.stack.iter().any(|s| matches!(s, Screen::Warning(_))),
            "an already-acknowledged recording error stays quiet",
        );
    }

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
        app.state.heading_up = false;
        tick_with(&mut app, Fix::at(0, 0), 200.0);
        assert_eq!(app.state.compass_deg, None);
    }

    #[test]
    fn peak_view_adopts_compass_even_with_a_cached_moving_fix_and_north_up_map() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.state.heading_up = false;
        assert!(app.ui.stack.push(Screen::PeakView(crate::screen::PeakViewScreen::new(app.state.user_fix))).is_ok());
        app.ui.map_dirty = false;
        tick_with(&mut app, moving(45.0), 215.0);
        assert_eq!(app.state.compass_deg, Some(215.0));
        assert!(app.ui.map_dirty, "a stopped compass turn repaints the visible panorama");
    }

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

    /// Feed one altitude sample through `App::tick`'s altimeter arm.
    fn tick_alt(app: &mut App, alt_m: f32, now_ms: u32) {
        let mut loc = OneFix(None); // no fix this tick — isolate the altimeter path
        let mut alt = OneAlt(Some(alt_m));
        app.tick(RideClock(now_ms), Sensors { altimeter: Some(&mut alt), ..Sensors::new(&mut loc) }, None);
    }

    #[test]
    fn tick_integrates_barometric_climb_dead_banded() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // boots Riding
        tick_alt(&mut app, 100.0, 1000); // anchor
        assert_eq!(app.recorder.climb_m(), 0.0, "the first sample only anchors");
        tick_alt(&mut app, 102.0, 2000); // +2 m: inside the dead-band
        assert_eq!(app.recorder.climb_m(), 0.0, "sub-dead-band noise books nothing through tick");
        tick_alt(&mut app, 110.0, 3000); // +10 m from the 100 m reference
        assert_eq!(app.recorder.climb_m(), 10.0, "a clean climb books through the full tick path");
    }

    #[test]
    fn tick_does_not_book_climb_while_paused() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        tick_alt(&mut app, 100.0, 1000); // anchor while riding
        tick_alt(&mut app, 110.0, 2000); // +10 m climbed
        assert_eq!(app.recorder.climb_m(), 10.0);

        app.activity.mode = Mode::Paused; // as a `press` → Ride control would set it
        tick_alt(&mut app, 160.0, 3000); // +50 m of drift during the stop
        tick_alt(&mut app, 160.0, 4000);
        assert_eq!(app.recorder.climb_m(), 10.0, "no climb accrues across a paused tick");

        app.activity.mode = Mode::Riding; // resume
        tick_alt(&mut app, 160.0, 5000); // re-anchors at the current height
        tick_alt(&mut app, 165.0, 6000); // a real +5 m after resuming
        assert_eq!(app.recorder.climb_m(), 15.0, "only genuine post-resume climb adds through tick");
    }

    /// A terrain source at a constant height that counts every sample taken from it.
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

    /// The barometer reads 75 m too high, 1875 against the map's 1800.
    #[test]
    fn tick_fuses_the_altimeter_onto_the_map_frame() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.test_start_ride(); // a ride, so the staged samples show what was actually recorded
        let mut terrain = FlatTerrain { height_m: 1800, samples: 0 };
        // Before any terrain sample the tile is the plain barometric reading.
        pass(&mut app, &mut terrain, Some(fix_at(0)), 1875.0, 1000);
        assert_eq!(app.recorder.current_elevation_m(), Some(1875.0), "unsettled → the raw reading");

        for i in 1..40 {
            pass(&mut app, &mut terrain, Some(fix_at(i)), 1875.0, 1000 + i * 1000);
        }
        let shown = app.recorder.current_elevation_m().expect("a sample has arrived");
        assert!((shown - 1800.0).abs() < 1.0, "the tile now reads the map-referenced height, got {shown}");
        assert_eq!(app.recorder.baro_elevation_m(), Some(1875.0), "the raw barometric reading is untouched");
        let staged = app.recorder.staged();
        assert_eq!(staged.first().map(|p| p.ele), Some(1875), "the RECORDED elevation stays raw barometry");
        assert!(app.recorder.altitude().settled());
    }

    #[test]
    fn terrain_is_sampled_once_per_fix_never_per_frame() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let mut terrain = FlatTerrain { height_m: 900, samples: 0 };
        pass(&mut app, &mut terrain, Some(fix_at(0)), 910.0, 1000);
        assert_eq!(terrain.samples, 1, "one fix, one sample");
        for i in 0..10 {
            assert!(!app.sample_terrain(&mut terrain), "no fresh fix → nothing to sample");
            pass(&mut app, &mut terrain, None, 911.0 + i as f32, 2000 + i * 100);
        }
        assert_eq!(terrain.samples, 1, "still exactly one terrain read");
        pass(&mut app, &mut terrain, Some(fix_at(1)), 910.0, 9000);
        assert_eq!(terrain.samples, 2, "the next fresh fix takes exactly one more");
    }

    #[test]
    fn a_terrain_less_map_leaves_the_elevation_tile_exactly_as_it_was() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let mut null = obc_elevation::NullElevation;
        for i in 0..60 {
            pass(&mut app, &mut null, Some(fix_at(i)), 640.0 + i as f32, 1000 + i * 1000);
        }
        assert!(!app.recorder.altitude().settled(), "no residual ever arrived");
        assert_eq!(app.recorder.altitude().offset_m(), None);
        assert_eq!(app.recorder.current_elevation_m(), app.recorder.baro_elevation_m());
        assert_eq!(app.recorder.current_elevation_m(), Some(699.0));
    }

    /// A heart-rate strap that yields one sample then runs dry.
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

    /// The drains run before `record_fix`, so the same tick's moving interval books the fresh
    /// samples. A drain moved after the fix would read `None` here.
    #[test]
    fn tick_drains_ble_sensors_into_live_values_and_summaries() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // boots Riding
        const STEP_UD: i32 = 45; // ~5 m of latitude — one second at ~5 m/s, comfortably moving

        // t = 1 s: the motion anchor, no sensor samples yet — everything reads `--`.
        tick_fix(&mut app, Fix::at(0, 0), 1_000);
        assert_eq!(app.recorder.live_hr(1_000), None, "no sample yet → no live HR");
        assert_eq!(app.recorder.avg_hr(), None);

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

        assert_eq!(app.recorder.live_hr(2_000), Some(150), "tick drained the HR strap");
        assert_eq!(app.recorder.live_power(2_000), Some(250), "tick drained the power meter");
        assert_eq!(app.recorder.live_cadence(2_000), Some(90), "tick drained the cadence sensor");
        assert_eq!(app.recorder.avg_hr(), Some(150), "the moving interval booked the fresh HR");
        assert_eq!(app.recorder.max_hr(), Some(150));
        assert_eq!(app.recorder.avg_power(), Some(250), "…and the fresh power");
        assert_eq!(app.recorder.max_power(), Some(250));
        assert_eq!(app.recorder.avg_cadence(), Some(90), "…and the fresh cadence");
    }

    /// The stat tiles judge sensor freshness with the `live_*_display` accessors, which compare
    /// against the last `tick`'s `RideClock`, not the render-time `self.ui.now_ms`. On the board
    /// those are one monotonic clock; in the simulator mid replay they diverge.
    #[test]
    fn sensor_tile_display_survives_render_clock_divergence() {
        // The sample records on playback time (30 s) while the map-plane clock runs on wall
        // time (90 s): a 60 s gap, wider than `SENSOR_STALE_MS`.
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.ui.now_ms = 90_000; // wall clock, far ahead of the replay's playback clock
        let mut loc = OneFix(None);
        let mut hr = OneHr(Some(142));
        app.tick(
            RideClock(30_000), // playback time — the clock the HR sample records on
            Sensors { hr: Some(&mut hr), ..Sensors::new(&mut loc) },
            None,
        );
        assert_eq!(app.recorder.live_hr_display(), Some(142), "the tile shows the value across the divergence");
        // The render-clock read is the one that goes stale.
        assert_eq!(
            app.recorder.live_hr(app.ui.now_ms),
            None,
            "the render-clock read is stale (90 s vs a 30 s sample) — the bug `_display` fixes"
        );

        // Staleness still works on the ride clock: 6 s past the sample with no new reading.
        app.recorder.note_sensor_clock(36_001);
        assert_eq!(app.recorder.live_hr_display(), None, "a >5 s-old sample still blanks — no frozen value");
    }

    /// One pass with only an HR sample (no fix, nothing else moving): `loc` yields `None`, so
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

    /// The grid's row declares the live sensor values in its render key, so a changed displayed
    /// value repaints the grid once and the 5 s staleness expiry moves the key too.
    #[test]
    fn fresh_sensor_sample_repaints_the_riding_view() {
        let mut app = on_statistics(crate::stat_fields::StatFieldList::decode(
            1,
            &[crate::stat_fields::StatField::HeartRate as u8],
        ));

        assert!(pass_hr_only(&mut app, Some(155), 1_000).map, "a fresh HR sample must repaint the grid");
        assert!(!pass_hr_only(&mut app, Some(155), 2_000).map, "an unchanged displayed value must not re-dirty");
        assert!(pass_hr_only(&mut app, Some(156), 3_000).map, "a changed bpm repaints again");

        // The strap drops: more than 5 s later the staleness gate blanks the tile.
        assert!(pass_hr_only(&mut app, None, 9_001).map, "the staleness expiry (value → `--`) must repaint");
        assert!(!pass_hr_only(&mut app, None, 20_000).map, "still blank → no re-dirty");
    }

    #[test]
    fn sensor_sample_without_a_pinned_tile_never_repaints() {
        let mut app =
            on_statistics(crate::stat_fields::StatFieldList::decode(1, &[crate::stat_fields::StatField::Speed as u8]));
        assert!(!pass_hr_only(&mut app, Some(155), 1_000).map, "no HR tile pinned → no forced render");
    }

    /// Home's key names battery, link and backdrop, and no sensor value.
    #[test]
    fn sensor_sample_on_home_never_repaints() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0)); // base = Home
        assert!(app.settings.stat_fields.push(crate::stat_fields::StatField::HeartRate));
        pass_idle(&mut app, 0); // drain the boot frame
        assert!(!pass_hr_only(&mut app, Some(155), 1_000).map, "Home draws no tiles → no repaint");
    }

    /// The Map draws chips, the route line and the marker, never a sensor tile, so a pinned HR
    /// field must not wake a map render at the strap's notification rate.
    #[test]
    fn sensor_sample_on_the_map_never_repaints() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map] — a map base
        assert!(app.settings.stat_fields.push(crate::stat_fields::StatField::HeartRate));
        pass_idle(&mut app, 0); // drain the boot frame
        assert!(!pass_hr_only(&mut app, Some(155), 1_000).map, "the Map draws no tiles → no repaint");
    }

    /// The debounce coalesces a multi-step edit into one write.
    #[test]
    fn a_settings_edit_flags_dirty_on_leaving_the_settings_subtree() {
        use crate::settings::Units;
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        // Walk to the Units row: Settings list → System (the last group) → Units (its first row).
        app.apply_gesture(Gesture::BackHold); // Home → Menu
        app.apply_gesture(Gesture::Step(-1)); // → Settings entry (wraps back from Routes)
        app.apply_gesture(Gesture::Press); // → Settings list
        app.apply_gesture(Gesture::Step(-1)); // → System row (last, wraps up from Ride)
        app.apply_gesture(Gesture::Press); // → System page (Units is the first row)
        app.apply_gesture(Gesture::Press); // → the Units editor sheet, over the page
        assert!(!settings_dirty(&mut app), "navigation changed no setting, so nothing to save");

        let before = app.settings().units;
        app.apply_gesture(Gesture::Step(1)); // stage Imperial
        assert_eq!(app.settings().units, before, "staging commits nothing");
        app.apply_gesture(Gesture::Press); // commit: the sheet closes onto the System page
        assert_ne!(app.settings().units, before, "the editor flipped the system");
        assert_eq!(app.settings().units, Units::Imperial, "default Metric → Imperial");
        assert!(!settings_dirty(&mut app), "still on a settings screen → the save is held, not fired per step");

        app.apply_gesture(Gesture::Back); // System page → Settings list (still inside the settings subtree)
        assert!(!settings_dirty(&mut app), "the Settings list is itself a settings screen — save stays held");

        // A sheet over a settings page is still inside the subtree: the pending edit stays held
        // while the editor is up, and while the quick drawer is.
        app.apply_gesture(Gesture::Step(-1)); // → System row
        app.apply_gesture(Gesture::Press); // → System page
        app.apply_gesture(Gesture::Press); // → the Units editor sheet
        assert!(!settings_dirty(&mut app), "the editor sheet over a settings page holds the save");
        app.apply_gesture(Gesture::Back); // close the sheet
        assert!(app.apply_chord(crate::input::Chord::Quick));
        assert!(!settings_dirty(&mut app), "the quick drawer over a settings page holds the save too");
        app.apply_gesture(Gesture::Back); // close the drawer
        app.apply_gesture(Gesture::Back); // System page → Settings list

        app.apply_gesture(Gesture::Back); // Settings list → Menu (left the settings subtree)
        assert!(settings_dirty(&mut app), "leaving settings flushes the pending edit — one coalesced save");
        assert!(!settings_dirty(&mut app), "and the flag drains — only saved once");
    }

    /// Every settings screen holds a pending save while it is the top screen and flushes it once
    /// on exit. A new settings screen whose `screens!` row forgets `=> Settings` would flush
    /// mid-edit and fail its case here.
    #[test]
    fn every_settings_screen_holds_a_pending_save_until_exit() {
        use crate::screen::settings::page;
        use crate::screen::{apply, AddFieldScreen, ResetScreen, SettingsPage, StatFieldsScreen, Transition};
        use crate::settings::Units;

        /// The screens to stack on the Home root (bottom first — parents under children, as the
        /// real navigation leaves them) and the gesture script performing one edit on the top one.
        type Case = (&'static str, fn() -> heapless::Vec<Screen, 2>, &'static [Gesture]);
        fn one(s: Screen) -> heapless::Vec<Screen, 2> {
            let mut v = heapless::Vec::new();
            let _ = v.push(s);
            v
        }
        let cases: [Case; 10] = [
            // Pure navigation — no edit gesture of its own.
            ("Settings list", || one(Screen::Settings(SettingsPage::hub())), &[]),
            // Open the UTC-offset editor sheet over the page, step it and commit; the sheet pops
            // and the page is on top again.
            (
                "Date & Time",
                || one(Screen::DateTime(SettingsPage::new(&page::DATETIME))),
                &[Gesture::Press, Gesture::Step(1), Gesture::Press],
            ),
            // The Units row: open its editor, step to imperial, commit.
            (
                "System",
                || one(Screen::System(SettingsPage::new(&page::SYSTEM))),
                &[Gesture::Press, Gesture::Step(1), Gesture::Press],
            ),
            // → the Auto-flip row (index 1), open its editor, +1 s, commit.
            (
                "Ride",
                || one(Screen::Ride(SettingsPage::new(&page::RIDE))),
                &[Gesture::Step(1), Gesture::Press, Gesture::Step(1), Gesture::Press],
            ),
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
            // The Bluetooth switch, flipped.
            ("Connections", || one(Screen::Connections(SettingsPage::new(&page::CONNECTIONS))), &[Gesture::Press]),
            // → the Power Saver row, flip it.
            ("Power", || one(Screen::Power(SettingsPage::new(&page::POWER))), &[Gesture::Step(1), Gesture::Press]),
            // Pure navigation — the Firmware page's install action leaves the settings subtree.
            ("Firmware", || one(Screen::Firmware(SettingsPage::new(&page::FIRMWARE))), &[]),
            // Press arms, then the completed hold erases to defaults — a real diff off the seed below.
            ("Reset", || one(Screen::Reset(ResetScreen::new())), &[Gesture::Press, Gesture::Hold]),
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

            // Back out to the Home root, closing any open field on the way.
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

    /// Upper bound of `Back` presses needed to unwind any settings case above, so a regression
    /// cannot loop forever.
    const MAX_DEPTH_BACKOUT: usize = crate::screen::MAX_DEPTH;

    /// A host-pushed warning still lands over the deepest ordinary mid-ride settings path. This
    /// walks that one path with gestures; how deep a rider can get at all, and what that leaves,
    /// belong to `the_deepest_descent_stops_short_of_max_depth`.
    #[test]
    fn deepest_mid_ride_settings_path_keeps_room_for_host_warning() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.set_routes_with_ids(&[summary("Alpha")], &[10]);

        app.apply_gesture(Gesture::Press); // Home → Menu (Routes selected)
        app.apply_gesture(Gesture::Press); // Menu → Route menu
        app.apply_gesture(Gesture::Press); // Route menu → Overview
        app.apply_gesture(Gesture::Press); // START → [Home, Map]
        assert!(matches!(app.top_screen(), Screen::Map(_)));

        app.apply_gesture(Gesture::BackHold); // the global escape: Map → Menu (Push, ride caller kept)
        app.apply_gesture(Gesture::Step(-1)); // Routes → Settings
        app.apply_gesture(Gesture::Press); // Menu → Settings
        app.apply_gesture(Gesture::Press); // Settings → Ride (Data fields is the first row)
        app.apply_gesture(Gesture::Press); // Ride → Fields
        let field_count = app.settings().stat_fields.len();
        app.apply_gesture(Gesture::Step(field_count as i32)); // first field → trailing Add tile
        app.apply_gesture(Gesture::Press); // Fields → Add field

        assert!(matches!(app.top_screen(), Screen::AddField(_)), "the deepest normal path is open");
        assert_eq!(app.ui.stack.len(), 7, "the full mid-ride settings path occupies seven slots");

        app.on_warning(WarningFlags::REC_ERROR);
        assert_eq!(app.ui.stack.len(), 8, "the host warning pushes over the deepest normal path");
        match app.top_screen() {
            Screen::Warning(w) => assert!(w.flags().contains(WarningFlags::REC_ERROR)),
            _ => panic!("the recording-error warning must not be dropped at maximum normal depth"),
        }
    }

    #[test]
    fn set_settings_does_not_flag_dirty() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let seeded = crate::settings::Settings { units: crate::settings::Units::Imperial, ..Default::default() };
        app.set_settings(seeded);
        assert_eq!(app.settings().units, crate::settings::Units::Imperial);
        assert!(!settings_dirty(&mut app), "seeding the boot value must not trigger a write-back");
    }

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

    /// The UTC offset is the one clock edit a settings screen still makes, and it re-stamps the
    /// wall clock to local time.
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
        app.apply_gesture(Gesture::Press); // open the offset editor sheet
        app.apply_gesture(Gesture::Step(1)); // +one step (+15 min), staged
        app.apply_gesture(Gesture::Press); // commit
        assert_eq!(app.settings().utc_offset_min, crate::settings::UTC_OFFSET_STEP, "the offset stepped one step");
        let now = app.wall_clock_now();
        assert_eq!((now.hour, now.minute), (12, 15), "the offset re-stamped the wall clock to local = UTC + offset");
    }

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

    /// The per-category cache asks for a corridor snapshot only where the answer is read, and
    /// the request is a single-category one anchored at live progress. Everywhere else the
    /// scratch stays disarmed and the board never builds a `Reader` for it.
    #[test]
    fn next_category_tiles_ask_for_a_corridor_only_where_they_are_drawn() {
        use crate::stat_fields::{StatField, StatFieldList};
        use obc_reader::{PoiCategory, PoiCategorySet};

        let mut app = App::new(AppState::new(0, 0, 1.0)); // base = the riding Map
        app.test_start_ride();
        app.navigator.route_state_mut().active_route = Some(0);
        app.navigator.route_state_mut().progress_m = 1_500;

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
            Some(crate::corridor::CorridorKey {
                hours_filter: obc_reader::reader::places::HoursFilter::HideClosed,
                filter: PoiCategorySet::only(PoiCategory::Water),
                anchor_m: 1_500
            }),
            "one category per query (the 16-result cap makes a union query unable to answer)"
        );
        assert!(app.corridor_snapshot_pending(), "…and the host is asked for the Reader until it lands");

        // Leave the stats page: the request goes away with it, Reader seam quiet again.
        app.apply_gesture(Gesture::Back);
        assert!(!matches!(app.top_screen(), Screen::Statistics(_)));
        app.advance_animations(InputClock(4_000));
        assert!(!app.corridor_snapshot_pending(), "a tile nobody is looking at costs nothing");

        // A route-less ride never asks either — there is no "ahead" to answer with.
        app.navigator.route_state_mut().active_route = None;
        app.apply_gesture(Gesture::Back);
        app.advance_animations(InputClock(5_000));
        assert!(!app.corridor_snapshot_pending());
    }

    /// A screen always outranks the stat-field cache for the one shared corridor scratch. The
    /// cache's harvest only accepts its own key, so the list's snapshot cannot land in a tile.
    #[test]
    fn an_up_ahead_screen_outranks_the_stat_field_cache() {
        use crate::stat_fields::{StatField, StatFieldList};
        use obc_reader::{PoiCategory, PoiCategorySet};

        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.test_start_ride();
        app.navigator.route_state_mut().active_route = Some(0);
        app.navigator.route_state_mut().progress_m = 2_000;
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

        app.open_whats_next();
        app.apply_gesture(Gesture::Press);
        assert!(matches!(app.top_screen(), Screen::WhatsNext(_)));
        app.advance_animations(InputClock(2_000));
        assert_eq!(
            app.ui.corridor_scratch.armed().map(|k| k.filter),
            Some(PoiCategorySet::ALL),
            "the screen's own key wins the shared buffer"
        );
    }

    /// The whole loop through real frames: the cache arms a single-category corridor request,
    /// the pre-draw `prepare` boundary runs the query off the map `Reader`, and the tile reads
    /// it. The query count is the point: the same seam per frame would be an SD read per frame.
    #[test]
    fn the_next_category_cache_fills_from_a_real_frame_and_then_goes_quiet() {
        use crate::stat_fields::{StatField, StatFieldList};
        use embedded_graphics::pixelcolor::Rgb888;
        use obc_formats::io::SliceSource;
        use obc_reader::{MapCache, MapTables, PoiCategory, Reader};
        use obc_route::{RouteIndex, RouteReader};
        use obcm_testkit::{build_poi_map, PoiSpec};

        use crate::harness::support::VecSink;

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

        // A due-east route with water points beside it, about 1.7 km and 3.5 km along.
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
                    PoiSpec { lat: 48_001_000, lon: 7_823_000, subtype: 1, name: "Brunnen".into(), payload: 0xFFFF },
                    PoiSpec { lat: 47_999_000, lon: 7_847_000, subtype: 1, name: "Spring".into(), payload: 0xFFFF },
                ],
            )],
        );

        // One rendered frame with both inputs — the shape the board produces when
        // `base_needs_reader` says it must.
        let frame = |app: &mut App, geometry: Option<&[u8]>| {
            let cache = MapCache::new();
            let map_src = SliceSource(&map);
            let tables = MapTables::parse(&map_src).expect("valid .obcm");
            let reader = Reader::new(&map_src, &tables, &cache);
            let route_src = SliceSource(geometry.unwrap_or(&obcr));
            let idx = RouteIndex::read(&route_src).expect("valid .obcr");
            let route = RouteReader::new(&idx, &route_src);
            let mut scratch = Box::new(RenderScratch::new());
            app.render_frame(Some(&mut scratch), &mut Sink, &reader, geometry.map(|_| &route), 240.0, 320.0, |_| {
                Rgb888::new(0, 0, 0)
            });
        };

        let mut app = App::new(AppState::new(7_800_000, 48_000_000, 0.05));
        app.test_start_ride();
        app.navigator.route_state_mut().active_route = Some(0);
        let mut fields = StatFieldList::decode(0, &[]);
        assert!(fields.push(StatField::NextWater));
        app.set_settings(crate::settings::Settings { stat_fields: fields, ..Default::default() });
        app.apply_gesture(Gesture::Back); // Map → Statistics
        assert!(matches!(app.top_screen(), Screen::Statistics(_)));

        app.advance_animations(InputClock(1_000));
        assert!(app.base_needs_reader(), "the armed refresh keeps the Reader built");
        frame(&mut app, Some(&obcr));
        assert!(!app.base_needs_reader(), "…exactly until the snapshot lands, then it stops");
        let first = app.ui.next_ahead.poi(PoiCategory::Water).expect("the nearest water ahead is cached");
        assert_eq!(first.name.as_str(), "Brunnen", "entry 0 of a single-category query is the nearest");
        let brunnen_m = first.dist_along_m;
        assert!((1_500..2_000).contains(&brunnen_m), "…projected onto the route axis, got {brunnen_m}");

        // Ride on inside the step: many frames, not one further query.
        for m in (100..500).step_by(50) {
            app.navigator.route_state_mut().progress_m = m;
            app.advance_animations(InputClock(2_000 + m));
            assert!(!app.base_needs_reader(), "no re-query inside the refresh step (at {m} m)");
            frame(&mut app, Some(&obcr));
        }
        assert_eq!(app.ui.next_ahead.poi(PoiCategory::Water).map(|p| p.dist_along_m), Some(brunnen_m));

        // Cross the step: exactly one re-take, and the answer is unchanged (nothing was passed).
        app.navigator.route_state_mut().progress_m = crate::next_ahead::REFRESH_STEP_M;
        app.advance_animations(InputClock(9_000));
        assert!(app.base_needs_reader(), "crossing the step re-arms");
        frame(&mut app, Some(&obcr));
        assert!(!app.base_needs_reader(), "and settles again on the very next eligible frame");
        assert_eq!(app.ui.next_ahead.poi(PoiCategory::Water).map(|p| p.dist_along_m), Some(brunnen_m));

        // Ride past the cached fountain: the re-take hands the tile the next one along.
        app.navigator.route_state_mut().progress_m = brunnen_m + 10;
        app.advance_animations(InputClock(10_000));
        frame(&mut app, Some(&obcr));
        assert_eq!(
            app.ui.next_ahead.poi(PoiCategory::Water).map(|p| p.name.as_str().into()),
            Some(std::string::String::from("Spring")),
            "a passed entry re-arms out of turn and the next one takes its place"
        );
    }

    /// A same-index, new-bytes route replace invalidates the `Next: <category>` cache. The cache
    /// keys its identity on the catalog index, and a replace leaves the index and the id exactly
    /// where they were, so nothing inside `NextAhead` can see the swap. Progress is pinned at 0,
    /// the one case the progress-rewind trigger cannot cover.
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
                poi: Poi {
                    opening: Default::default(),
                    metadata: Default::default(),
                    lat: 0,
                    lon: 0,
                    subtype: 1,
                    name,
                    hours_ref: 0xFFFF,
                    distance_m: 5_000,
                },
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
            Some(crate::corridor::CorridorKey {
                hours_filter: obc_reader::reader::places::HoursFilter::HideClosed,
                filter: PoiCategorySet::only(PoiCategory::Water),
                anchor_m: 0
            }),
            "…and the identical route index re-queries against the new bytes"
        );
    }

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

    #[test]
    fn place_detail_minute_and_unavailable_list_inputs_do_not_poll_at_one_millisecond() {
        use obc_reader::{MapCache, MapTables, Poi, PoiCategory, Reader, SliceSource};
        let bytes = obcm_testkit::build_poi_map((0, 0, 1_000_000, 1_000_000), 512, &[]);
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.settings.idle_return = crate::settings::IdleReturn::Never;
        app.stamp_clock(DateTime { year: 2025, month: 1, day: 6, hour: 12, minute: 0 }, 0, Some(0), ClockTrust::Ble);
        app.ui.stack.clear();
        let _ = app.ui.stack.push(Screen::PoiList(screen::PoiListScreen::new(PoiCategory::Water)));
        let fix = obc_ports::Fix::at(500_000, 500_000);
        for (map, position) in [(None, Some(fix)), (Some(&reader), None)] {
            app.advance_animations(InputClock(0));
            app.ui.prepare_base(map, None, position, None, 0, 0, &[], app.place_local_time());
            assert_ne!(app.ms_until_next_wake(0), Some(1), "missing input must await an external update");
        }
        app.ui.prepare_base(Some(&reader), None, Some(fix), None, 0, 0, &[], app.place_local_time());
        let _ = app.ui.stack.push(Screen::PoiDetail(screen::PoiDetailScreen::new(Poi {
            metadata: Default::default(),
            opening: Default::default(),
            lat: fix.lat,
            lon: fix.lon,
            subtype: 1,
            name: heapless::String::new(),
            hours_ref: 0xffff,
            distance_m: 0,
        })));
        app.ui.prepare_base(Some(&reader), None, Some(fix), None, 0, 0, &[], app.place_local_time());
        for now in [60_000, 60_001, 120_000] {
            app.advance_animations(InputClock(now));
            app.ui.prepare_base(None, None, Some(fix), None, 0, 0, &[], app.place_local_time());
            assert!(app.ms_until_next_wake(now).is_none_or(|ms| ms > 1), "cached detail only needs minute updates");
        }
    }

    #[test]
    fn find_banner_and_wake_cover_the_complete_candidate_batch() {
        use crate::find_place::{Action, State};
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.apply_chord(Chord::Assistant);
        app.apply_gesture(Gesture::Press);
        app.apply_gesture(Gesture::Press);
        app.advance_animations(InputClock(0));
        assert!(app.find_preparing());
        assert_eq!(app.ms_until_next_wake(0), Some(1));
        assert!(matches!(app.planning_banner(), Some(Msg::AssistantFinding)));
        app.take_dirty();
        app.ui.find.action = Action::None;
        for state in [State::Querying, State::Planning] {
            app.ui.find.state = state;
            app.mode.search_started(PlanFamily::Route);
            assert!(!app.take_dirty().overlay, "a new candidate does not replace the banner");
            app.mode.search_ended(PlanFamily::Route);
            assert!(!app.reroute_freeze_active(), "arena admission still follows the actual planner");
            assert!(matches!(app.planning_banner(), Some(Msg::AssistantFinding)));
            assert!(!app.take_dirty().overlay, "releasing a candidate does not clear the banner");
            assert_eq!(app.ms_until_next_wake(0), Some(1), "queued work cannot wait for another GPS fix");
        }
        app.advance_animations(InputClock(999));
        assert!(!app.take_dirty().overlay, "activity does not repaint before its deadline");
        app.advance_animations(InputClock(1_000));
        assert!(app.take_dirty().overlay, "activity repaints at one second");
        assert!(!app.take_dirty().overlay, "activity never repaints twice in one phase");
        app.advance_animations(InputClock(1_500));
        assert!(!app.take_dirty().overlay);
        assert_eq!(app.ui.next_wake_ms, Some(500));
        app.ui.find.state = State::Ready;
        app.advance_animations(InputClock(1_500));
        assert!(app.ui.next_wake_ms.is_none_or(|ms| ms > 1_000), "ready results stop the activity timer");
        assert!(app.planning_banner().is_none());
        assert!(app.take_dirty().overlay, "the completed batch clears its banner");
        assert_ne!(app.ms_until_next_wake(1_500), Some(1), "a ready result does not poll");
        app.ui.find.state = State::Planning;
        app.apply_gesture(Gesture::Back);
        assert!(app.planning_banner().is_none(), "leaving choices removes the banner");
    }

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

    // The pure hysteresis resolvers are pinned in `navigator/following.rs`, next to the policy
    // they encode. Here the App-side wiring is driven end to end.

    use obc_formats::io::SliceSource;
    use obc_route::RouteIndex;

    /// The committed Grimsel fixture bytes: 3 back-to-back climbs at 501-11067, 11067-14472 and
    /// 14472-18547 m, about 18.7 km total.
    const GRIMSEL: &[u8] = include_bytes!("../../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr");

    /// Parse the fixture into a `RouteIndex` the callers pair with a `SliceSource` over [`GRIMSEL`].
    fn grimsel_index() -> RouteIndex {
        let src = SliceSource(GRIMSEL);
        RouteIndex::read(&src).unwrap()
    }

    /// Pin the composed route-following result through the App tick. The in-RAM route carries
    /// elevation for one climb, named waypoints, an off-route excursion, and a fix at the end.
    #[test]
    fn composed_guidance_trace_is_stable() {
        use crate::harness::support::VecSink;

        const LAT: f64 = 48.0;
        const LON: f64 = 7.8;
        let mut gpx = std::string::String::from(r#"<?xml version="1.0"?><gpx version="1.1">"#);
        for (point, name) in [(6u32, "Bridge"), (14, "Pass"), (24, "Water"), (34, "Village")] {
            let lon = LON + point as f64 * 0.002;
            gpx.push_str(&std::format!(r#"<wpt lat="{LAT:.4}" lon="{lon:.4}"><name>{name}</name></wpt>"#));
        }
        gpx.push_str("<trk><trkseg>");
        for point in 0u32..40 {
            let lon = LON + point as f64 * 0.002;
            let rise = point.clamp(12, 28) - 12;
            let elevation = 200.0 + rise as f64 * (300.0 / 16.0);
            gpx.push_str(&std::format!(r#"<trkpt lat="{LAT:.4}" lon="{lon:.4}"><ele>{elevation:.1}</ele></trkpt>"#));
        }
        gpx.push_str("</trkseg></trk></gpx>");

        let mut sink = VecSink::default();
        obc_route::gpx_to_obcr(&SliceSource(gpx.as_bytes()), "Guidance", &mut sink).unwrap();
        let src = SliceSource(sink.0.as_slice());
        let index = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&index, &src);
        assert_eq!(route.detect_climbs().len(), 1, "the in-memory route contains one detected climb");

        let summary = RouteSummary::read(&src).unwrap();
        let mut app = App::new(AppState::new((LON * 1e6) as i32, (LAT * 1e6) as i32, 1.0));
        app.set_routes_with_ids(&[summary], &[1]);
        app.activate_route(0);

        let mut trace = std::vec::Vec::new();
        for (step, off_route) in [
            (0u32, false),
            (5, false),
            (6, false),
            (7, false),
            (12, false),
            (15, true),
            (16, false),
            (24, false),
            (25, false),
            (28, false),
            (34, false),
            (35, false),
            (39, false),
        ] {
            let lon = LON + step as f64 * 0.002;
            let lat = if off_route { LAT + 0.02 } else { LAT };
            let mut loc = OneFix(Some(Fix::at((lat * 1e6) as i32, (lon * 1e6) as i32)));
            app.tick(RideClock(step * 1_000 + 1), Sensors::new(&mut loc), Some(&route));
            trace.push((
                app.navigator.route_state_mut().progress_m,
                app.navigator.route_state_mut().off_route,
                app.navigator.route_state_mut().dist_to_route_m,
                app.navigator.route_state_mut().active_climb,
                app.navigator.route_state_mut().next_waypoint,
            ));
        }

        assert_eq!(
            trace,
            [
                (0, false, 0, Some(0), Some(0)),
                (744, false, 0, Some(0), Some(0)),
                (893, false, 0, Some(0), Some(0)),
                (1_042, false, 0, Some(0), Some(1)),
                (1_787, false, 0, Some(0), Some(1)),
                (1_787, true, 2_226, Some(0), Some(1)),
                (2_383, false, 0, Some(0), Some(2)),
                (3_575, false, 0, Some(0), Some(2)),
                (3_724, false, 0, Some(0), Some(3)),
                (4_171, false, 0, Some(0), Some(3)),
                (5_065, false, 0, None, Some(3)),
                (5_214, false, 0, None, None),
                (5_810, false, 0, None, None),
            ]
        );
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
        app.navigator.route_state_mut().active_route = Some(0);
        app.test_start_ride();
        tick_without_fix(&mut app, Some(&route)); // establish route/session caches

        app.navigator.route_state_mut().progress_m = 1_000;
        let session = app.ride_session();
        let mode = app.activity.mode;
        app.navigator.request_seam(0, 12_000); // lands on the fixture's second climb
        tick_without_fix(&mut app, Some(&route));
        assert_eq!(app.navigator.route_state_mut().progress_m, 12_000);
        assert!(!app.navigator.route_state_mut().off_route);
        assert_eq!(
            app.navigator.route_state_mut().active_climb,
            Some(1),
            "climb guidance re-derived at the new anchor"
        );
        assert!(!app.navigator.pending_seam());
        assert_eq!(app.ride_session(), session);
        assert_eq!(app.activity.mode, mode);

        // A fix in the skipped stretch cannot pull matching behind the durable floor.
        let p = route.position_at(2_000).unwrap();
        let mut loc = OneFix(Some(Fix { lon: p.lon, lat: p.lat, course: None, speed_mps: None }));
        app.tick(RideClock(1_000), Sensors::new(&mut loc), Some(&route));
        assert!(app.navigator.route_state_mut().off_route);
        assert_eq!(app.navigator.route_state_mut().progress_m, 12_000);

        // Removing/reloading the route resets the floor; an early fix on the reloaded geometry can
        // establish an early first lock again (the tracking session itself is still the same).
        app.navigator.route_state_mut().active_route = None;
        tick_without_fix(&mut app, None);
        app.navigator.route_state_mut().active_route = Some(0);
        tick_without_fix(&mut app, Some(&route));
        let mut loc = OneFix(Some(Fix { lon: p.lon, lat: p.lat, course: None, speed_mps: None }));
        app.tick(RideClock(2_000), Sensors::new(&mut loc), Some(&route));
        assert!(!app.navigator.route_state_mut().off_route);
        assert!(app.navigator.route_state_mut().progress_m < 3_000, "route reload cleared the old 12 km floor");

        // A new session on the same route also clears every seam-derived anchor.
        app.navigator.request_seam(0, 8_000);
        tick_without_fix(&mut app, Some(&route));
        assert_eq!(app.navigator.route_state_mut().progress_m, 8_000);
        app.test_end_ride();
        app.test_start_ride();
        tick_without_fix(&mut app, Some(&route));
        assert_eq!(app.navigator.route_state_mut().progress_m, 0);
        assert!(!app.navigator.route_state_mut().off_route);
    }

    #[test]
    fn failed_seam_seek_keeps_old_anchor_and_retries() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let idx = grimsel_index();
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);
        app.navigator.route_state_mut().active_route = Some(0);
        app.test_start_ride();
        tick_without_fix(&mut app, Some(&route));
        app.navigator.route_state_mut().progress_m = 1_000;
        app.navigator.request_seam(0, 4_000);

        let empty = SliceSource(&[]);
        let unreadable = RouteReader::new(&idx, &empty);
        tick_without_fix(&mut app, Some(&unreadable));
        assert_eq!(
            app.navigator.route_state_mut().progress_m,
            1_000,
            "failed decode does not split route state from matcher"
        );
        assert!(app.navigator.pending_seam(), "transient failure remains retryable");

        tick_without_fix(&mut app, Some(&route));
        assert_eq!(app.navigator.route_state_mut().progress_m, 4_000);
        assert!(!app.navigator.pending_seam());
    }

    fn app_with_detour_chooser_on_beta() -> App {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.state.has_nav_graph = true;
        app.state.user_fix = Some(Fix { lon: 7_800_000, lat: 48_000_000, course: None, speed_mps: None });
        app.set_routes_with_ids(&[summary("Alpha"), summary("Beta"), summary("Gamma")], &[10, 20, 30]);
        app.navigator.route_state_mut().active_route = Some(1);
        app.navigator.route_state_mut().progress_m = 1_000;
        app.navigator.route_state_mut().route_total_m = 5_000;
        app.test_start_ride();
        let chooser = crate::screen::DetourScreen::new(app.navigator.route_state());
        *app.ui.stack.last_mut().unwrap() = Screen::Detour(chooser);
        app
    }

    #[test]
    fn detour_chooser_and_queued_plan_follow_route_identity_across_rescans() {
        let mut app = app_with_detour_chooser_on_beta(); // Beta id 20 at index 1
        app.set_routes_with_ids(&[summary("Gamma"), summary("Alpha"), summary("Beta")], &[30, 10, 20]);
        assert_eq!(app.navigator.route_state_mut().active_route, Some(2), "active navigation followed Beta to index 2");

        app.apply_gesture(Gesture::Press);
        assert_eq!(app.navigator.pending_detour_request().unwrap().route, 2, "the open chooser followed Beta too");

        // Before the host drains the request, another rescan moves Beta again.
        app.set_routes_with_ids(&[summary("Beta"), summary("Gamma"), summary("Alpha")], &[20, 30, 10]);
        assert_eq!(app.navigator.route_state_mut().active_route, Some(0));
        assert_eq!(app.navigator.pending_detour_request().unwrap().route, 0, "the queued plan request follows Beta");
    }

    #[test]
    fn vanished_detour_route_disables_the_chooser_and_clears_a_queued_plan() {
        let mut open = app_with_detour_chooser_on_beta();
        open.set_routes_with_ids(&[summary("Alpha"), summary("Gamma")], &[10, 30]);
        assert_eq!(open.navigator.route_state_mut().active_route, None, "vanished Beta unloads navigation");
        open.apply_gesture(Gesture::Press);
        assert!(matches!(open.top_screen(), Screen::Detour(_)), "an unavailable chooser stays safely cancellable");
        assert!(open.navigator.pending_detour_request().is_none(), "it never retargets the route now at old index 1");

        let mut queued = app_with_detour_chooser_on_beta();
        queued.apply_gesture(Gesture::Press);
        assert!(queued.navigator.pending_detour_request().is_some());
        queued.set_routes_with_ids(&[summary("Alpha"), summary("Gamma")], &[10, 30]);
        assert!(queued.navigator.pending_detour_request().is_none(), "a queued plan for vanished Beta is cancelled");
    }

    /// Drive the active climb with a controlled `progress_m` over the real fixture reader, which
    /// isolates the refill from the matcher's fix-snapping.
    #[test]
    fn update_active_climb_refills_exactly_on_entry_transitions() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let idx = grimsel_index();
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);
        *app.navigator.climbs_mut() = route.detect_climbs();
        assert_eq!(app.navigator.climbs().len(), 3, "the Grimsel fixture segments into 3 climbs");

        // Sweep progress across the whole route in 250 m steps. Climb boundaries (from the fixture):
        // 501–11067, 11067–14472, 14472–18547 — three entries as the sweep crosses each base.
        let mut entries = 0;
        let mut prev = None;
        for p in (0..=18_725u32).step_by(250) {
            app.navigator.route_state_mut().progress_m = p;
            app.update_active_climb(&route);
            if app.navigator.route_state_mut().active_climb != prev
                && app.navigator.route_state_mut().active_climb.is_some()
            {
                entries += 1;
            }
            prev = app.navigator.route_state_mut().active_climb;
        }
        // The climbs are back-to-back, so the sweep enters all three: 3 entries, 3 fills.
        assert_eq!(entries, 3, "the sweep enters each of the 3 climbs once");
        assert_eq!(
            app.navigator.climb_fill_count(),
            3,
            "the detail buffer is rebuilt exactly on the 3 entries, not per fix"
        );
    }

    #[test]
    fn update_active_climb_freezes_while_off_route() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let idx = grimsel_index();
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);
        *app.navigator.climbs_mut() = route.detect_climbs();

        // On climb 0 (progress mid-first-climb).
        app.navigator.route_state_mut().progress_m = 5000;
        app.update_active_climb(&route);
        assert_eq!(app.navigator.route_state_mut().active_climb, Some(0));
        let fills_on_climb = app.navigator.climb_fill_count();

        // Go off-route: progress freezes (the matcher holds it). Even a progress value that would
        // otherwise be past every climb must not change the active climb while off-route.
        app.navigator.route_state_mut().off_route = true;
        app.navigator.route_state_mut().progress_m = 99_999;
        app.update_active_climb(&route);
        assert_eq!(app.navigator.route_state_mut().active_climb, Some(0), "off-route holds the current climb");
        assert_eq!(app.navigator.climb_fill_count(), fills_on_climb, "no refill while off-route");
    }

    // These reuse the Grimsel fixture and step `progress_m` across a base or summit to fire the
    // transition, then inspect which screen is on top.

    /// A Riding-view app with the fixture's climbs loaded and a given climb mode.
    fn climb_app(mode: crate::settings::ClimbMode) -> (App, RouteIndex) {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // stack [Home, Map], Riding
        app.settings.climb_mode = mode;
        let idx = grimsel_index();
        {
            let src = SliceSource(GRIMSEL);
            let route = RouteReader::new(&idx, &src);
            *app.navigator.climbs_mut() = route.detect_climbs();
        }
        (app, idx)
    }

    /// Enter a climb (drive progress across climb 0's base) via `update_active_climb`.
    fn enter_first_climb(app: &mut App, idx: &RouteIndex) {
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(idx, &src);
        app.navigator.route_state_mut().progress_m = 5_000; // mid climb 0 (501–11067)
        app.update_active_climb(&route);
        assert_eq!(app.navigator.route_state_mut().active_climb, Some(0), "the fixture puts progress on climb 0");
    }

    #[test]
    fn auto_switches_to_climb_on_entry_from_a_riding_view() {
        use crate::settings::ClimbMode;
        let (mut app, idx) = climb_app(ClimbMode::Auto);
        assert!(matches!(app.top_screen(), Screen::Map(_)), "starts on the Map (a riding view)");
        enter_first_climb(&mut app, &idx);
        assert!(matches!(app.top_screen(), Screen::Climb(_)), "Auto auto-shows the Climb screen on entry");
    }

    #[test]
    fn auto_never_switches_away_from_a_menu() {
        use crate::screen::MenuScreen;
        use crate::settings::ClimbMode;
        let (mut app, idx) = climb_app(ClimbMode::Auto);
        let _ = app.ui.stack.push(Screen::Menu(MenuScreen::new()));
        assert!(
            !matches!(app.top_screen(), Screen::Map(_) | Screen::Statistics(_)),
            "the top is a menu, not one of the two views the entry edge replaces"
        );
        enter_first_climb(&mut app, &idx);
        assert!(matches!(app.top_screen(), Screen::Menu(_)), "the menu is left untouched by the entry edge");
        assert!(
            matches!(app.ui.stack[app.ui.stack.len() - 2], Screen::Map(_)),
            "the base riding view is untouched too"
        );
    }

    /// The rider pulls the card: the next scan answers with no free-space figure, and the row
    /// goes back to `--` rather than keeping a count read off a card that is gone.
    #[test]
    fn a_card_scan_with_no_figure_blanks_the_free_space_row() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.storage.note_measured(Some(8 * 1024 * 1024));
        assert_eq!(app.storage.free_bytes(), Some(8 * 1024 * 1024), "the scan answered");

        app.storage.note_measured(None);
        assert_eq!(app.storage.free_bytes(), None, "and a scan with no figure leaves the rider a `--`");
    }

    /// The preview polyline is derived from the detour plan, so the plan takes it along whichever
    /// way the rider leaves the preview: Back on it, or the Assistant chord, which drops the whole
    /// detour flow off the stack and leaves no screen that could answer for the plan.
    #[test]
    fn leaving_a_detour_drops_its_preview_polyline() {
        use crate::screen::{DetourPreviewScreen, DetourScreen};
        /// A ride on a route, with a detour planned and its preview landed, as the flow does it.
        fn previewing() -> App {
            let mut app = App::new(AppState::new(0, 0, 1.0));
            app.set_routes_with_ids(&[summary("Road")], &[7]);
            app.state.has_nav_graph = true;
            app.state.user_fix = Some(Fix { lon: 7_800_000, lat: 48_000_000, course: None, speed_mps: None });
            app.navigator.route_state_mut().active_route = Some(0);
            app.navigator.route_state_mut().progress_m = 1_000;
            app.navigator.route_state_mut().route_total_m = 20_000;
            app.test_start_ride();

            let chooser = DetourScreen::new(app.navigator.route_state());
            let preview = crate::host::DetourPreview {
                cost_delta_m: 420,
                total_distance_m: 1_220,
                rejoin_m: 2_000,
                ascent_m: None,
            };
            app.admit_navigator_intent(NavigatorIntent::PlanDetour(crate::activity::DetourRequest {
                route: 0,
                from: (7_800_000, 48_000_000),
                progress_m: 1_000,
                target_m: 1_800,
                leg: obc_route::Leg::Detour,
            }));
            let _ = app.ui.stack.push(Screen::Detour(chooser));
            let _ = app.ui.stack.push(Screen::DetourPreview(DetourPreviewScreen::new(&chooser, preview)));
            app.set_detour_preview(&[(7_812_000, 48_001_000), (7_816_000, 48_001_000)]);
            assert!(!app.catalogs.detour_preview_for(Some(0)).is_empty(), "the host's shape is cached");
            app
        }

        let mut app = previewing();
        app.apply_gesture(Gesture::Back); // the rider drops the detour
        assert!(
            app.catalogs.detour_preview_for(Some(0)).is_empty(),
            "and the shape goes with the plan, not one frame later"
        );

        let mut app = previewing();
        assert!(app.apply_chord(Chord::Assistant));
        assert!(!app.navigator.detour_planned(), "the chord took the plan with the screens that answer for it");
        assert!(app.catalogs.detour_preview_for(Some(0)).is_empty(), "…and its shape off the map");

        let mut app = previewing();
        app.set_backlight_available(true);
        assert!(app.apply_chord(Chord::Quick));
        app.advance_animations(InputClock(2_000)); // settle the sheet's open slide
        for _ in 0..2 {
            app.apply_gesture(Gesture::Step(1)); // brightness → bluetooth → settings
        }
        app.apply_gesture(Gesture::Press);
        assert!(matches!(app.top_screen(), Screen::Settings(_)), "the drawer's row drops the detour flow too");
        assert!(!app.navigator.detour_planned(), "…and takes the plan with it");
    }

    /// The Detour chooser is an interaction in progress, not an auto-switch sibling, so a climb
    /// entry must preserve both it and its selected distance.
    #[test]
    fn auto_never_switches_away_from_the_detour_chooser() {
        use crate::screen::DetourScreen;
        use crate::settings::ClimbMode;
        let (mut app, idx) = climb_app(ClimbMode::Auto);
        app.state.has_nav_graph = true;
        app.state.user_fix = Some(Fix { lon: 7_800_000, lat: 48_000_000, course: None, speed_mps: None });
        app.navigator.route_state_mut().active_route = Some(0);
        app.navigator.route_state_mut().progress_m = 1_000;
        app.navigator.route_state_mut().route_total_m = 20_000;
        app.test_start_ride();
        let chooser = DetourScreen::new(app.navigator.route_state());
        *app.ui.stack.last_mut().unwrap() = Screen::Detour(chooser);
        app.apply_gesture(Gesture::Step(2)); // selected distance = the 600 m minimum + 200 m

        enter_first_climb(&mut app, &idx); // live anchor advances to 5 km on climb entry
        assert!(matches!(app.top_screen(), Screen::Detour(_)), "climb entry preserves the open chooser");

        app.apply_gesture(Gesture::Press);
        let req = app.navigator.pending_detour_request().expect("the preserved chooser still plans");
        assert_eq!((req.route, req.target_m), (0, 5_800), "the 800 m selection survives the climb edge");
    }

    #[test]
    fn manual_and_off_never_auto_switch_on_entry() {
        use crate::settings::ClimbMode;
        for mode in [ClimbMode::Manual, ClimbMode::Off] {
            let (mut app, idx) = climb_app(mode);
            enter_first_climb(&mut app, &idx);
            assert!(matches!(app.top_screen(), Screen::Map(_)), "{mode:?} leaves the rider on the Map on entry");
        }
    }

    #[test]
    fn crest_auto_returns_to_map_from_the_climb_screen() {
        use crate::settings::ClimbMode;
        let (mut app, idx) = climb_app(ClimbMode::Auto);
        enter_first_climb(&mut app, &idx); // Auto → now on the Climb screen
        assert!(matches!(app.top_screen(), Screen::Climb(_)));
        // Jump progress past the last climb's exit band so the active climb clears (Some → None).
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);
        app.navigator.route_state_mut().progress_m = 50_000;
        app.update_active_climb(&route);
        assert_eq!(app.navigator.route_state_mut().active_climb, None, "past every climb → no active climb");
        assert!(matches!(app.top_screen(), Screen::Map(_)), "the crest returns to the Map from the Climb screen");
    }

    #[test]
    fn crest_repairs_a_hidden_climb_below_the_detour_chooser() {
        use crate::settings::ClimbMode;
        let (mut app, idx) = climb_app(ClimbMode::Auto);
        app.state.has_nav_graph = true;
        // The ride opens first, because a session start zeroes the ride.
        app.test_start_ride();
        enter_first_climb(&mut app, &idx); // top = Climb
        app.navigator.route_state_mut().active_route = Some(0);
        app.navigator.route_state_mut().route_total_m = 50_000;

        assert!(app.apply_chord(crate::input::Chord::Context)); // [Home, Climb, ContextDrawer]
        app.apply_gesture(Gesture::Step(1)); // → the Detour row
        app.apply_gesture(Gesture::Press); // the row replaces the sheet → [Home, Climb, Detour]
        assert!(matches!(app.top_screen(), Screen::Detour(_)));

        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);
        app.navigator.route_state_mut().progress_m = 50_000;
        app.update_active_climb(&route);
        assert_eq!(app.navigator.route_state_mut().active_climb, None);
        assert!(matches!(app.top_screen(), Screen::Detour(_)), "crest does not dismiss the chooser");
        assert!(
            matches!(app.ui.stack[app.ui.stack.len() - 2], Screen::Map(_)),
            "the hidden Climb caller is repaired in place"
        );

        app.apply_gesture(Gesture::Back);
        assert!(matches!(app.top_screen(), Screen::Map(_)), "Back reveals the repaired riding caller");
    }

    #[test]
    fn crest_leaves_other_screens_untouched() {
        use crate::screen::MenuScreen;
        use crate::settings::ClimbMode;
        let (mut app, idx) = climb_app(ClimbMode::Manual); // Manual: entry won't switch
        enter_first_climb(&mut app, &idx);
        let _ = app.ui.stack.push(Screen::Menu(MenuScreen::new())); // now on a menu, mid-climb
        let src = SliceSource(GRIMSEL);
        let route = RouteReader::new(&idx, &src);
        app.navigator.route_state_mut().progress_m = 50_000;
        app.update_active_climb(&route);
        assert_eq!(app.navigator.route_state_mut().active_climb, None);
        assert!(matches!(app.top_screen(), Screen::Menu(_)), "a crest never yanks a menu to the Map");
    }

    /// It goes through `tick`, not the internal setter, to exercise the real load/unload path.
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
        assert!(app.navigator.climbs().is_empty(), "no active route → no climbs, even with a reader present");
        assert!(app.navigator.cache_keys().0.is_none());
        assert!(
            app.navigator.waypoints().is_empty() && app.navigator.cache_keys().1.is_none(),
            "no active route → no waypoint table"
        );
        assert_eq!(
            app.navigator.route_state_mut().waypoint_count,
            0,
            "the gesture-side table length mirrors the empty cache"
        );

        // GRIMSEL carries no waypoints, so the table stays empty but the build key advances:
        // that is how the load shows it ran.
        app.navigator.route_state_mut().active_route = Some(0);
        no_loc(&mut app, Some(&route));
        assert_eq!(app.navigator.climbs().len(), 3, "an active route + reader segments the climbs on load");
        assert_eq!(app.navigator.cache_keys().0, Some(0));
        assert_eq!(app.navigator.cache_keys().1, Some(0), "the waypoint table loads on the same route edge");
        let waypoint_count = app.navigator.route_state().waypoint_count;
        assert_eq!(
            waypoint_count,
            app.navigator.waypoints().len(),
            "the gesture-side table length mirrors the loaded resident cache"
        );

        app.navigator.route_state_mut().active_climb = Some(0); // pretend we were on a climb
        app.navigator.route_state_mut().next_waypoint = Some(0); // …and had a next waypoint
        app.navigator.route_state_mut().active_route = None;
        no_loc(&mut app, None);
        assert!(app.navigator.climbs().is_empty(), "unloading the route clears the climbs");
        assert!(app.navigator.cache_keys().0.is_none());
        assert_eq!(app.navigator.route_state_mut().active_climb, None, "and the on-climb state is dropped");
        assert!(
            app.navigator.waypoints().is_empty() && app.navigator.cache_keys().1.is_none(),
            "unloading clears the waypoint table"
        );
        assert_eq!(app.navigator.route_state_mut().next_waypoint, None, "and the next-waypoint index is dropped");
        assert_eq!(
            app.navigator.route_state_mut().waypoint_count,
            0,
            "and the gesture-side table length clears with it"
        );
    }

    // The idle sweep runs in `advance_animations`; these tests set `last_input_ms`, push a
    // screen, then advance the clock past the deadline and inspect the top screen.

    use crate::screen::{
        MenuScreen, NavPlanningScreen, PasskeyScreen, RouteReceivedScreen, SettingsPage, StatisticsScreen,
        WarningFlags, WarningScreen,
    };
    use crate::settings::IdleReturn;

    /// Run one idle sweep at `now_ms` — the same path `advance_animations` takes, at a chosen clock.
    fn idle_tick(app: &mut App, now_ms: u32) {
        app.advance_animations(InputClock(now_ms));
    }

    #[test]
    fn idle_returns_to_home_when_not_tracking() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0)); // [Home], Idle
        app.settings.idle_return = IdleReturn::S30;
        let _ = app.ui.stack.push(Screen::Menu(MenuScreen::new()));
        let _ = app.ui.stack.push(Screen::Settings(SettingsPage::hub()));
        app.ui.last_input_ms = 0;

        idle_tick(&mut app, 29_000); // still inside the window
        assert!(matches!(app.top_screen(), Screen::Settings(_)), "no return before the deadline");

        idle_tick(&mut app, 30_000); // deadline reached
        assert_eq!(app.ui.stack.len(), 1, "cleared to the Home root");
        assert!(matches!(app.top_screen(), Screen::Home(_)), "and the top is Home");
    }

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

    /// It elapses to 20 s: past the 15 s idle deadline, but under the route popup's own 30 s
    /// auto-close, so only the idle exemption is under test.
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

    /// The route-less browse map is a deliberate view, so it is exempt even though it is not the
    /// Home root.
    #[test]
    fn browse_map_is_exempt_from_idle_return() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0)); // Idle, not tracking
        app.settings.idle_return = IdleReturn::S15;
        let _ = app.ui.stack.push(Screen::Map(MapScreen::new())); // the browse map over Home
        app.ui.last_input_ms = 0;
        idle_tick(&mut app, 60_000);
        assert!(matches!(app.top_screen(), Screen::Map(_)), "the browse map is a deliberate view — never yanked");
        // The browse map's only pending wake is the start hint's auto-hide; once that window
        // elapses it arms no wake at all.
        idle_tick(&mut app, 60_000 + 4_000);
        assert_eq!(app.ms_until_next_wake(60_000 + 4_000), None, "and it arms no idle wake");

        // A menu over Home, by contrast, does return.
        *app.ui.stack.last_mut().unwrap() = Screen::Menu(MenuScreen::new());
        app.ui.last_input_ms = 60_000;
        app.ui.idle_return_timing = true; // model the gesture that opened the menu
        idle_tick(&mut app, 120_000);
        assert!(matches!(app.top_screen(), Screen::Home(_)), "a menu still returns to Home on the timeout");
    }

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

    #[test]
    fn idle_return_arms_a_wake() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.settings.idle_return = IdleReturn::S30;
        let _ = app.ui.stack.push(Screen::Menu(MenuScreen::new()));
        app.ui.last_input_ms = 0;
        idle_tick(&mut app, 10_000);
        assert_eq!(app.ms_until_next_wake(10_000), Some(20_000), "wake armed 20 s out (30 s − 10 s elapsed)");
    }

    #[test]
    fn dfu_install_request_is_take_once() {
        use crate::activity::DfuAction;
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        assert_eq!(drain_dfu(&mut app), None, "nothing pending at boot");
        app.dfu.admit_intent(crate::dfu::DfuIntent::InstallRequested);
        assert_eq!(drain_dfu(&mut app), Some(DfuAction::Install), "the posted request drains");
        assert_eq!(drain_dfu(&mut app), None, "…exactly once");
    }

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

    #[test]
    fn ride_track_request_hands_out_the_id_until_answered() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.set_rides(&[
            crate::RideEntry { id: 7, summary: ride_summary("A") },
            crate::RideEntry { id: 9, summary: ride_summary("B") },
        ]);

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
        app.set_rides(&[crate::RideEntry { id: 9, summary: ride_summary("B") }]);
        assert_eq!(app.activity.viewed_ride, Some(0), "the viewed index follows the id");
        assert_eq!(ride_track_request(&app), None, "the answer moved with it");

        // The viewed ride itself vanishing clears the keys — nothing left to request.
        app.set_rides(&[crate::RideEntry { id: 7, summary: ride_summary("A") }]);
        assert_eq!(app.activity.viewed_ride, None);
        assert_eq!(ride_track_request(&app), None);
    }

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

    /// The DFU slot is most-recent-wins by design: one phase in flight, and a later rider post
    /// supersedes it.
    #[test]
    fn dfu_slot_is_most_recent_wins() {
        use crate::activity::DfuAction;
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.dfu.admit_intent(crate::dfu::DfuIntent::ScanRequested);
        app.dfu.admit_intent(crate::dfu::DfuIntent::InstallRequested);
        assert_eq!(drain_dfu(&mut app), Some(DfuAction::Install), "the later phase superseded");
        assert_eq!(drain_dfu(&mut app), None);
    }

    #[test]
    fn persist_settings_waits_for_subtree_exit_and_is_single_sourced() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let _ = app.ui.stack.push(Screen::Settings(crate::screen::SettingsPage::hub()));
        app.arm_settings_save(); // rev → 1
        assert!(!settings_dirty(&mut app), "still editing — nothing owed yet");

        app.ui.stack.pop(); // leave the subtree
        assert_eq!(drain_persist(&mut app), Some(1));
        assert!(!settings_dirty(&mut app), "one emit, then Awaiting — no second pending state");
    }

    // These drive the revision handshake through the domain's own seam: `SettingsMachine` hands
    // out one write and validates the answer's operation token and revision independently.

    #[test]
    fn no_persist_during_a_stepper_sweep_inside_the_subtree() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let _ = app.ui.stack.push(Screen::Settings(crate::screen::SettingsPage::hub()));
        // A sweep of edits while inside the subtree: several revisions, but never an emit.
        for _ in 0..5 {
            app.arm_settings_save();
            assert_eq!(drain_persist(&mut app), None, "held while a settings screen is on top");
        }
        app.ui.stack.pop(); // leave the subtree
        assert_eq!(drain_persist(&mut app), Some(5), "one coalesced emit for the latest revision");
        assert_eq!(drain_persist(&mut app), None, "and only once — now Awaiting the ack");
    }

    #[test]
    fn persist_success_clears_the_dirty_state() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let mut host = SettingsHost::default();
        app.arm_settings_save();
        assert_eq!(host.drain(&mut app), Some(1));
        host.ack(&mut app, 1);
        assert_eq!(host.drain(&mut app), None, "acked → Clean, nothing owed");
    }

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

    #[test]
    fn repeated_failure_is_paced_by_the_backoff_window() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let mut host = SettingsHost::default();
        app.ui.now_ms = 1_000;
        app.arm_settings_save();
        for round in 0..3 {
            assert_eq!(host.drain(&mut app), Some(1), "one emit at the start of round {round}");
            host.fail(&mut app, 1);
            for _ in 0..4 {
                app.ui.now_ms += 100;
                assert_eq!(host.drain(&mut app), None, "no re-emit inside the backoff window");
            }
            app.ui.now_ms += SETTINGS_RETRY_BACKOFF_MS; // cross into the next window
        }
    }

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

    #[test]
    fn reboot_load_seeds_clean() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.arm_settings_save(); // pretend a stale dirty state survived somehow
        app.set_settings(Settings::default()); // boot seed (store load or default)
        assert_eq!(drain_persist(&mut app), None, "a seeded boot value is already persisted");
    }

    /// A cancel posted while the plan request is still undrained annihilates it: the rider's net
    /// intent is "no plan", so the host cannot execute a dismissed plan.
    #[test]
    fn a_cancel_annihilates_an_undrained_plan_request() {
        use crate::activity::NavRequest;

        // Confirm + Back in one batch cancels before any physical work.
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.admit_navigator_intent(NavigatorIntent::PlanRoute(NavRequest::new((0, 0), (1, 1), "A")));
        app.admit_navigator_intent(NavigatorIntent::CancelPlan);
        assert_eq!(drain_nav(&mut app), None, "annihilated before any host saw it");
        assert!(!drain_cancel(&mut app), "nothing was acquired, so no release is owed");

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

    // The four rules for a level-triggered read, exercised through the real seam: a need repeats
    // until answered, a failure answers it, an input for a key that is no longer current changes
    // nothing, and changing the subject creates a new key.

    /// A `set_rides` catalog with one ride at durable id `7`, and its detail opened.
    fn viewing_ride(ids: &[crate::CatalogObjectId]) -> App {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let rides: heapless::Vec<RideEntry, 4> = ids
            .iter()
            .map(|&id| RideEntry { id, summary: ride_summary(if id == 7 { "First" } else { "Second" }) })
            .collect();
        app.set_rides(&rides);
        app.activity.viewed_ride = Some(0);
        app
    }

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

    /// A need is not its subject: closing the Route overview ends the nav-preview level even
    /// though the route stays active.
    #[test]
    fn an_answer_that_lands_after_the_overview_closed_is_refused() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.set_routes_with_ids(&[summary("Col")], &[10]);
        app.navigator.route_state_mut().active_route = Some(0);
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

    /// A ride on the spliced rest of Day 2 and Day 3 is a ride on Day 3, and where it ends maps
    /// back onto the days: inside the rest it is a place on Day 2, past the join a place on Day 3.
    #[test]
    fn a_ride_on_the_rest_of_the_day_before_counts_for_the_day() {
        use crate::navigator::NavigatorIntent;
        let mut app = rest_ride_app();
        app.navigator.admit_intent(NavigatorIntent::PlanDetour(crate::DetourRequest::rest(1, 54_000, 74_000, 3_000)));
        let preview =
            crate::host::DetourPreview { cost_delta_m: 0, total_distance_m: 20_000, rejoin_m: 3_000, ascent_m: None };
        app.navigator.note_lead_preview(&preview);
        app.navigator.adopt_lead_in(99, 30);
        app.navigator.route_state_mut().active_route = Some(2);
        assert_eq!(app.ride_origin().trip, obc_formats::ride::TripRef::new(42, 2, 3), "Day 3 is the ride's day");
        app.recorder.set_origin(app.ride_origin());

        // A stop 5 km into the rest finishes Day 2, and Day 3 is still next.
        let record = finish_at(&mut app, 5_000);
        assert_eq!((record.day, record.day_route.id, record.metres, record.last_finished), (1, 20, 59_000, Some(1)));
        assert_eq!(app.next_trip_day().map(|(_, day)| day), Some(2));
    }

    /// After a reset the adopted lead-in is gone. The built day is still the active internal route,
    /// and the record and the line facts give its rest again, so the Finish lands on the same day.
    #[test]
    fn a_rest_ride_continued_after_a_reset_finishes_where_it_ended() {
        let mut app = rest_ride_app();
        app.set_internal_routes(1 << 2);
        app.navigator.route_state_mut().active_route = Some(2);
        app.recorder.set_origin(crate::RideOrigin {
            bike: obc_formats::bike::BikeType::Road,
            trip: obc_formats::ride::TripRef::new(42, 2, 3),
        });
        let record = finish_at(&mut app, 25_000);
        assert_eq!((record.day, record.day_route.id, record.metres, record.last_finished), (2, 30, 8_000, Some(2)));
    }

    /// A re-upload of the day's route during the ride voids the metres measured on the old one.
    #[test]
    fn a_day_route_replaced_during_the_ride_voids_the_metres() {
        let mut app = rest_ride_app();
        app.navigator.route_state_mut().active_route = Some(1);
        app.recorder.set_origin(app.ride_origin());
        app.metadata.begin_ride();
        app.on_route_uploaded(30, true, None);
        let record = finish_at(&mut app, 25_000);
        assert_eq!((record.day, record.metres, record.last_finished), (2, 0, Some(2)));
    }

    /// Day 2 (route 20) ridden to 54 km of 74, then Day 3 (route 30) and its built rest (99) in the
    /// catalog; Day 3 joins the line 3 km in.
    fn rest_ride_app() -> App {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.set_routes_with_ids(&[summary("Day 2"), summary("Day 3"), summary("From stop")], &[20, 30, 99]);
        app.set_trips(&[crate::trip::TripInput {
            id: 1,
            key: 42,
            name: "Alps",
            start_date: 0,
            stage_ids: &[10, 20, 30],
        }]);
        let record = crate::trip::TripProgress {
            key: 42,
            day: 1,
            day_route: crate::trip::RouteVersion { id: 20, revision: 1 },
            metres: 54_000,
            last_finished: Some(1),
            dates: [0; obc_route::MAX_TRIP_DAYS],
        };
        app.set_trip_progress([record]);
        app.set_day_join(Some(crate::trip::DayJoin { key: 42, day: 2, leave_m: 74_000, join_m: 3_000 }));
        app
    }

    fn finish_at(app: &mut App, progress_m: u32) -> crate::trip::TripProgress {
        app.navigator.route_state_mut().progress_m = progress_m;
        app.note_trip_finish();
        app.trip_progress().last().cloned().expect("the Finish writes the record")
    }

    #[test]
    fn internal_route_visibility_follows_catalog_identity_without_unloading_navigation() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.set_routes_with_ids(&[summary("Visit"), summary("Saved")], &[10, 20]);
        app.set_internal_routes(1);
        app.navigator.route_state_mut().active_route = Some(0);
        app.take_dirty();
        app.set_internal_routes(1);
        assert!(!app.take_dirty().map);
        app.set_routes_with_ids(&[summary("Saved"), summary("Visit")], &[20, 10]);
        assert_eq!(app.navigator.internal_routes(), 2);
        assert_eq!(app.active_route_index(), Some(1));
        assert_eq!(app.routes().len(), 2, "the durable navigation catalog stays complete");
    }

    #[test]
    fn repeated_catalog_feeds_do_not_repaint_but_changed_content_does() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let mut routes = [summary("Col")];
        let trips = [crate::trip::TripInput { id: 20, key: 1, name: "Tour", start_date: 0, stage_ids: &[10] }];
        let mut rides = [RideEntry { id: 30, summary: ride_summary("Ride") }];
        app.take_dirty();
        app.set_routes_with_ids(&routes, &[10]);
        assert!(app.take_dirty().map);
        app.set_trips(&trips);
        assert!(app.take_dirty().map);
        app.set_rides(&rides);
        assert!(app.take_dirty().map);
        app.set_routes_with_ids(&routes, &[10]);
        app.set_trips(&trips);
        app.set_rides(&rides);
        app.set_unaccepted_routes(0);
        assert!(!app.take_dirty().map, "an identical catalog feed changes no pixels");
        routes[0].climb_m += 1;
        app.set_routes_with_ids(&routes, &[10]);
        assert!(app.take_dirty().map);
        assert_eq!(app.trips()[0].climb_m, routes[0].climb_m);
        rides[0].summary.synced = true;
        app.set_rides(&rides);
        assert!(app.take_dirty().map);
        app.set_unaccepted_routes(1);
        assert!(app.take_dirty().map, "candidate visibility changes the route menu");
        app.set_unaccepted_routes(1);
        assert!(!app.take_dirty().map);
        app.set_trips(&[]);
        assert!(app.take_dirty().map);
    }

    /// A rider on Gravel, and two catalog routes the host would read as MTB.
    fn gravel_rider() -> App {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.set_routes_with_ids(&[summary("A"), summary("B")], &[10, 20]);
        app.settings.bike_type = crate::settings::BikeType::Gravel;
        app
    }

    /// Each rider load sets the current type from the loaded route, once: a start from the
    /// overview, Ride to start, a swap mid-ride, the received-route prompt, and a phone replace of the active
    /// route.
    #[test]
    fn a_route_load_sets_the_bike_type_once() {
        use crate::harness::support::tick_typed_route;
        use crate::settings::BikeType::{Mtb, Road};
        type Load = fn(&mut App);
        let loads: [(&str, Load); 5] = [
            ("Ride to start", |app| app.ride_approach(1)),
            ("start from the overview", |app| {
                app.navigator.route_state_mut().active_route = Some(0);
                let _ = app.ui.stack.push(Screen::RouteOverview(crate::screen::RouteOverviewScreen::new(0, None)));
                app.apply_gesture(Gesture::Press);
            }),
            ("swap mid-ride", |app| {
                app.test_start_ride();
                let _ = app.ui.stack.push(Screen::RouteSwap(crate::screen::RouteSwapScreen::new(1)));
                app.apply_gesture(Gesture::Press);
            }),
            ("received mid-ride", |app| {
                app.test_start_ride();
                let _ = app.ui.stack.push(Screen::RouteSwap(crate::screen::RouteSwapScreen::received(1, 0)));
                app.apply_gesture(Gesture::Press);
            }),
            ("phone replace of the active route", |app| {
                app.navigator.route_state_mut().active_route = Some(0);
                app.on_route_uploaded(10, true, None);
            }),
        ];
        for (what, load) in loads {
            let mut app = gravel_rider();
            load(&mut app);
            assert_eq!(tick_typed_route(&mut app, Mtb), Mtb, "{what} sets the route's type");
            assert!(settings_dirty(&mut app), "{what}: …and saves it");
            app.settings.bike_type = Road;
            assert_eq!(tick_typed_route(&mut app, Mtb), Road, "{what}: a rider change after the load stays");
        }
    }

    /// Changing the active route without a rider load keeps the rider's type: Back out of a browse
    /// preview, and a detour commit that splices the loaded route.
    #[test]
    fn an_active_route_change_that_is_not_a_load_keeps_the_rider_type() {
        use crate::harness::support::tick_typed_route;
        use crate::settings::BikeType::{Gravel, Mtb};
        let mut app = gravel_rider();
        app.navigator.route_state_mut().active_route = Some(1); // browsing B over the loaded A
        let _ = app.ui.stack.push(Screen::RouteOverview(crate::screen::RouteOverviewScreen::new(1, Some(0))));
        assert_eq!(tick_typed_route(&mut app, Mtb), Gravel, "a browse preview is not a load");
        app.apply_gesture(Gesture::Back);
        assert_eq!(app.active_route_index(), Some(0));
        assert_eq!(tick_typed_route(&mut app, Mtb), Gravel, "…and neither is Back to the loaded route");

        app.land_detour_commit(Ok(10));
        assert_eq!(tick_typed_route(&mut app, Mtb), Gravel, "a detour splice keeps the rider's type");
    }

    /// The one thing identity cannot catch: an upload that replaces a stored route keeps the
    /// identity and changes the geometry.
    #[test]
    fn a_replacing_upload_stales_the_nav_preview_under_the_same_route_id() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.set_routes_with_ids(&[summary("Col")], &[10]);
        app.navigator.route_state_mut().active_route = Some(0);
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
