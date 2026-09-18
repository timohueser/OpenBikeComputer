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
use obc_map_scene::MapScene;
use obc_ports::{Fix, InputClock, InputSource, LocationSource, RideClock, Sensors};

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
    /// The Free axis to come back to. Free is a stable mode family, not two adjacent stops in the
    /// mode ring, so leaving it for Route (or for Zoom, which keeps the axis) must not silently
    /// change direction.
    ///
    /// **Invariant: whenever [`basis`](Pan::basis) is a Free axis this equals it.** Only
    /// [`toggle_pan_free_axis`](crate::AppState::toggle_pan_free_axis) and
    /// [`enter_pan`](crate::AppState::enter_pan) choose a Free axis, and both write the pair
    /// together; the ring only ever reads it back. So the field carries a real value exactly while
    /// the basis is `Route` — and the ring needs no save of its own on the way out of Free.
    last_free_basis: PanBasis,
    /// A route step or basis change owes one cold `position_at` lookup at the pre-draw boundary.
    /// Private to the app: screens may inspect the mode, never acknowledge route I/O.
    route_camera_dirty: bool,
}

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
    /// map, so the orientation follows the compass instead of snapping to north. Peak View uses
    /// the same fallback even when the map preference is north-up. It is adopted only on ticks
    /// where one of those views would use it (see [`App::tick`]).
    pub compass_deg: Option<f32>,
    /// Installed terrain availability, captured observer framing, and current summit projections.
    /// The large panorama remains host-owned and is borrowed only while drawing.
    pub peak_view_profile: Option<crate::PeakViewProfile<'static>>,
    pub peak_view_peaks: [crate::PeakViewPeak; 64],
    pub peak_view_peak_count: u8,
    /// Small, current platform-fed facts rendered by ordinary app chrome.
    pub device: DeviceStatus,
    pub ble_forget_requested: bool,
    pub bond_status: crate::ble::BondStatus,
    /// Whether the loaded map carries a non-empty §8 nav graph (#882) — fed once at map open by
    /// [`App::set_map_nav_graph`]; the Detour station/chooser gate on it (a graph-less map dims
    /// the station instead of failing a plan). Carried here because both `handle` (the gate) and
    /// `draw` (the dimming) need it without a `Reader`.
    pub has_nav_graph: bool,

    /// The **Up-ahead timeline's category filter** — "Everything" ([`PoiCategorySet::ALL`]) or one
    /// of the six §7.4 categories.
    /// It resets to Everything on every entry to the list.
    ///
    /// It lives in the app plane rather than on
    /// [`WhatsNextScreen`](crate::screen::WhatsNextScreen) because the sheet that edits it (#1515 D4a)
    /// sits *above* that screen on the stack: a copy inside the screen would be a copy the rider's
    /// edit could not reach. Read with [`Settings::up_ahead_source`](crate::Settings) as one
    /// [`UpAheadScope`](crate::corridor::UpAheadScope).
    pub up_ahead_filter: obc_reader::PoiCategorySet,
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
            peak_view_profile: None,
            peak_view_peaks: [crate::PeakViewPeak::EMPTY; 64],
            peak_view_peak_count: 0,
            device: DeviceStatus {
                // Stand-in until a [`FuelGauge`](obc_ports::FuelGauge) feeds a real reading on the first tick.
                battery_pct: 75,
                // No phone linked until the host feeds the first [`BleStatus`](crate::BleStatus).
                ble_link: crate::BleLink::Advertising,
                ble_paired: false,
            },
            ble_forget_requested: false,
            bond_status: crate::ble::BondStatus::Idle,
            has_nav_graph: false,

            up_ahead_filter: obc_reader::PoiCategorySet::ALL,
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

    /// Advance the pan **mode ring** (Select tap): Route Move → Free Move → Zoom → Route Move.
    /// No-op when not panning.
    ///
    /// One ring replaced two gestures in #1515 D3. The movement family used to be a Back-hold, and
    /// Back-hold is the global escape now — so the family joined the tool on the tap that already
    /// switched modes, rather than moving onto a hold pan mode does not have spare. Without a route
    /// the ring is its two remaining stations, Free Move ↔ Zoom, which is exactly the toggle a
    /// route-less pan had before. Leaving Free remembers the axis, so a lap of the ring comes back
    /// to the axis the rider was using instead of silently changing it.
    pub fn cycle_pan_mode(&mut self, has_route: bool) {
        let Some(pan) = self.pan.as_mut() else { return };
        match (pan.tool, pan.basis) {
            // Route Move → Free Move, on the axis last used.
            (PanTool::Move, PanBasis::Route) => pan.basis = pan.last_free_basis,
            // Free Move → Zoom. The axis rides along in `basis`, and `last_free_basis` already
            // equals it (see the field's invariant), so there is nothing to save here.
            (PanTool::Move, _) => pan.tool = PanTool::Zoom,
            // Zoom → Route Move; with no route the ring closes straight back onto Free Move.
            (PanTool::Zoom, _) => {
                pan.tool = PanTool::Move;
                if has_route {
                    pan.basis = PanBasis::Route;
                    pan.route_camera_dirty = true;
                }
            }
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
    /// The camera / orientation / last-fix state — public so the host's mouse pan/zoom and control
    /// panel can read and adjust it directly.
    pub state: AppState,
    /// The operating mode, viewed ride, and small UI requests outside a product domain.
    pub activity: Activity,
    /// The resident route / ride / trip catalogs keyed by durable object ids, plus the
    /// identity-keyed view caches (ride profile/preview, nav preview) — the one component owning
    /// the id ↔ summary pairing and every rescan-remap invariant (#450, epic #526). Populated by
    /// the host through the `set_*` façade methods below.
    pub(crate) catalogs: CatalogState,
    /// The loaded map's routing-profile **names** (routing-v2 N5), refreshed by the host on map load
    /// ([`set_nav_profiles`](App::set_nav_profiles)) — resident because the bike-type editor
    /// and the created-route overview label render them on frames the host draws without a `Reader`.
    /// Only the names are mirrored (≤ 8 × 12 B); the multiplier tables stay solely in `MapTables`.
    nav_profiles: crate::NavProfiles,
    /// Private cadence and one-shot state used only by [`tick`](App::tick).
    tick_state: TickState,
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
    clock_trust: ClockTrust,
    pub recorder: crate::recorder::RecorderMachine,

    /// The **Navigator** domain: active-route following and caches, plus the rider's undelivered
    /// plan requests, per-family phase, and operation token. It is the only writer of route
    /// guidance and of [`mode`](App::mode)'s two search levels.
    pub(crate) navigator: NavigatorMachine,
    pub(crate) metadata: crate::metadata::MetadataMachine,
    pub(crate) easier: crate::easier::State,
    /// **`CoreMode`** (#1397 S5): the one owner of "what heavy work may run now, and what the rider
    /// is looking at" — the two search levels Navigator writes, the transfer level
    /// [`set_map_transfer`](App::set_map_transfer) writes, and the Recalculating banner's
    /// level→edge bit. Every reader of "a search is live" derives from it; nothing keeps a second
    /// copy.
    pub(crate) mode: CoreMode,
    /// The **settings-persistence** machine (#810, #1397 S2): the dirty revision, the subtree
    /// debounce, the retry backoff and the stale-answer rule.
    pub(crate) settings_ops: crate::settings::SettingsMachine,

    /// The **DFU** domain (#1397 S2): the single most-recent-wins update phase and its token.
    pub(crate) dfu: DfuState,
    pub(crate) bond: crate::ble::BondMachine,
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
            tick_state: TickState::new(),
            ui: UiRuntime::new() => UiRuntime::init_in_place,
            nav_profiles: crate::NavProfiles::new(),
            settings: Settings::default(),
            // The clock starts from the default set-point; the host re-stamps the persisted value.
            wall_clock: WallClock::new(Settings::default().local_clock()),
            // A persisted set-point is display-only until GPS or BLE establishes trust this boot.
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
        let App {
            state: camera,
            activity,
            catalogs,
            tick_state,
            ui,
            nav_profiles,
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
        assert!(nav_profiles.is_empty(), "no routing profiles before a map loads");
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
        self.recorder.note_sensor_clock(now_ms);
        // Whether the ride is accumulating at all. `Mode` is the route model's — a rider who pauses
        // stops the totals without ending the session, and a ride the card refused still shows its
        // distance — so Recorder is told rather than asked.
        let riding = self.activity.mode == Mode::Riding;
        // Read the freeze once, before anything below can move the stack: while a planner run holds
        // the arena over a map base, this tick must not advance route-match progress (see the fix
        // path below for why, and `device_core::core_mode` for the whole rule).
        let frozen = self.reroute_freeze_active();
        // The once-per-load route/session sync — matcher re-lock, session restart (accumulators +
        // breadcrumb), the route-length mirror, and the climbs/waypoints cache builds — is
        // Navigator's; a change there (route line appeared/vanished, breadcrumb cleared)
        // repaints the map even on a frame with no fresh fix.
        if self.navigator.sync_route_state(route) {
            self.ui.map_dirty = true;
        }
        // A detour commit queues a seam re-anchor because the commit handler owns no host
        // `RouteReader`. Install matcher progress + the forward-only floor at the splice seam
        // before this tick's fresh fix, then re-derive every guidance consumer from it.
        if self.navigator.apply_pending_seam(route) {
            if let Some(route) = route {
                self.update_active_climb(route);
                self.update_next_waypoint(route);
            }
            self.ui.map_dirty = true;
        }

        let Sensors { loc, altimeter, temperature, clock, compass, fuel, hr, power, cadence } = sensors;
        // Battery charge from the PMIC gauge, on the slow ~30 s cadence. Nothing here says
        // "repaint": the gauge is drawn by Home alone, Home's row declares
        // [`RenderKeyKind::Home`](crate::screen::RenderKeyKind) and that key carries the level — so
        // a change repaints exactly when Home is visible, and the riding views that never draw it
        // are not woken for a full ~97 ms map render every 30 s.
        if self.tick_state.battery_poll_due(now_ms) {
            if let Some(soc) = fuel.and_then(|f| f.poll()) {
                self.state.device.battery_pct = soc;
            }
        }
        // Barometric altitude → climb + the elevation stamped on the log. Polled before the fix so a
        // point logged this tick carries the freshest altitude.
        if let Some(altimeter) = altimeter {
            if let Some(alt) = altimeter.poll() {
                self.recorder.record_altitude(alt, riding);
            }
        }
        // Ambient temperature: on device the BMP581 reports it free alongside the per-fix pressure
        // read. Stored off `AppState` (no screen draws it yet) so it never gates a map redraw.
        if let Some(temperature) = temperature {
            if let Some(c) = temperature.poll() {
                self.tick_state.temp_c = Some(c);
            }
        }
        // BLE sensors → the live values Activity staleness-gates + the per-ride summaries. Drained
        // here beside the altimeter/temperature so `record_fix` (below, on a fresh fix) sees this
        // tick's samples. `Some` only on a fresh reading; a dropped strap simply stops reporting and
        // the staleness gate expires the last value. The stat tiles (SE5) read these through the
        // `live_*_display` accessors, and the Statistics grid's render key names those same values,
        // so a fresh sample repaints the grid — and only the grid.
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
            self.tick_state.last_fix_ms = Some(self.ui.now_ms);
            if let Some(Screen::PeakView(screen)) = self.ui.stack.iter_mut().rev().find(|screen| !screen.is_overlay()) {
                if screen.needs_position() {
                    screen.set_status(crate::peak_view::runtime::Status::Building(0));
                    self.ui.map_dirty = true;
                }
            }
            // Arm the map-referenced altimeter's one terrain read for this fix (EL8, epic #1068).
            // Nothing is sampled here — `tick` holds no elevation source, and an SD tile read does
            // not belong in the middle of the fix path anyway. The host drains it right after this
            // tick through `sample_terrain`, at the fix cadence and never per frame.
            self.tick_state.pending_terrain = Some((fix.lat, fix.lon));
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
                    self.navigator.note_unmatched_fix();
                } else {
                    self.navigator.match_fix(fix, route);
                }
                // "Am I on a climb now?" is derived from the fresh match — with hysteresis, and a
                // detail-profile refill only on a new climb entry (see `update_active_climb`).
                self.update_active_climb(route);
                // "Which waypoint is next?" from the same fresh progress — distance-lingered, and it
                // re-windows a truncated table forward as the rider advances (see below).
                self.update_next_waypoint(route);
            }

            // The fix into the ride: the totals, the trail and the sample the ride log owes. No
            // write happens here — the staged sample leaves as a
            // [`RecorderEffect::Append`](crate::recorder::RecorderEffect) at stage 7, so nothing on
            // the fix path touches a medium. `true` means the staging buffer was full and the log
            // lost a point, which the rider is told about exactly as a failed write is (issue #11);
            // `on_warning` latches it, so a whole ride of them is one dismissable card.
            if self.recorder.record_fix(fix, now_ms, riding) {
                self.on_warning(WarningFlags::REC_ERROR);
            }
        }
        // Electronic compass → the heading when the GPS can't give a course. Polled after the fix so
        // it sees this tick's movement state, and adopted *only* when it would actually drive the
        // orientation: a heading-up map that is not panning, or Peak View, and the latest fix has
        // no course (stopped). Peak View deliberately uses the same effective-heading chain as the
        // map, independent of the map's current north-up/heading-up preference.
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
    /// `elev` is the same [`ElevationSource`] the route emitter fills from: retained map terrain,
    /// or [`NullElevation`](obc_elevation::NullElevation) where there is none. With the
    /// null source (or outside the raster's coverage) the sample is `None`, nothing is fed, the
    /// estimator never settles, and the Elevation tile keeps its pre-epic barometric reading.
    ///
    /// It is deliberately **not** part of [`tick`](App::tick): `Sensors` is `obc-ports` vocabulary
    /// and terrain is not a sensor — it is the map, which the app already reaches through its own
    /// seam. Keeping it here also keeps the source's `&mut` out of the fix path, where the board
    /// holds it as a `.bss` `&'static mut` shared with the planner.
    pub fn sample_terrain(&mut self, elev: &mut dyn ElevationSource) -> bool {
        let Some((lat, lon)) = self.tick_state.pending_terrain.take() else { return false };
        if let Some(map_m) = elev.sample(lat, lon) {
            self.recorder.record_map_elevation(map_m);
        }
        true
    }
    /// Recompute Navigator's active climb from the freshly-matched progress — its hysteresis and
    /// once-per-entry detail refill — then apply the App-plane consequences of a
    /// transition: one repaint, and the C5 host auto-switch off the same edge.
    fn update_active_climb(&mut self, route: &RouteReader) {
        if let Some((prev, next)) = self.navigator.update_active_climb(route) {
            // No repaint request: the active climb is in the Statistics and Climb render keys, so
            // the riding views' climb-scoped readouts repaint from the declaration.
            // Host-driven auto-switch / auto-return (C5), off the same entry/exit edge.
            self.apply_climb_auto_switch(prev, next);
        }
    }

    /// Recompute Navigator's next waypoint from the freshly-matched progress — its linger
    /// hysteresis and truncated-table re-window — repainting once when the next waypoint moved.
    fn update_next_waypoint(&mut self, route: &RouteReader) {
        // No repaint request: the next waypoint is in the Map and Statistics render keys, so the
        // chip and the fields repaint from the declaration.
        self.navigator.update_next_waypoint(route);
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
    ///   repair matters when the ride context or Skip ahead was opened from Climb before the crest. Either
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

    /// Whether the **Recalculating freeze** is engaged (issue #1146, P2): a host planner run is
    /// live *and* the base screen would draw the map. While it is, a render-on-demand host must
    /// **skip the map redraw** — the last frame stays on the reflective glass — and paint only
    /// [`render_overlay`](App::render_overlay), which raises a planning banner over it.
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

    /// Whether the visible place choices are still being prepared.
    pub fn find_preparing(&self) -> bool {
        use crate::find_place::{Action, State};
        matches!(self.top_screen(), Screen::FindPlace(screen) if screen.choices())
            && (matches!(self.ui.find.action, Action::Refresh | Action::Preview(_))
                || matches!(self.ui.find.state, State::Start | State::Querying | State::Planning | State::Releasing))
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
            Some(if self.requested_assistant_restore().is_some() {
                Msg::AssistantLoadingRoute
            } else {
                Msg::AssistantPlanningRoute
            })
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

    /// Whether the frame this pass renders draws the **sheet and nothing else**: a drawer covers a
    /// base that does not recess, on a host that declared a
    /// [resident frame](App::set_resident_frame), so the base's draw is skipped (#1559).
    ///
    /// A host asks so it can *say so*: on the board an open step and a whole-screen redraw both
    /// cost a render and a push, and its RTT line is the only instrument either is measured with
    /// (#1569). A frame that skipped the base is not "a screen redraw" and must not claim to be.
    pub fn sheet_only(&self) -> bool {
        self.ui.sheet_only()
    }

    /// Whether there's a **current** GPS fix at `now_ms`: a fix has been accepted and is no older
    /// than the tick state's staleness window. `false` before the
    /// first fix (acquiring) and once the signal drops (lost) — exactly when the "No GPS Fix"
    /// banner shows.
    pub fn has_live_fix(&self, now_ms: u32) -> bool {
        self.tick_state.has_live_fix(now_ms, &self.settings)
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
    /// (the multiplier tables stay in `MapTables`); the bike-type editor steps them and the
    /// created-route overview labels itself with the selected one. Safe to call on a router-less
    /// (`ble`) image — the names are map metadata and the row still renders (inert). Dirties the
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
    /// Navigator's active route, the matcher/profile caches keyed on it, an open Skip-ahead
    /// chooser or queued skip commit, a Route-menu selection, a Route-overview preview, and a pending
    /// [`RouteSwapScreen`](crate::screen::RouteSwapScreen) at the *same route* (by id) in the new
    /// order. A vanished route falls back sanely: navigation unloads (`active_route = None`, stale
    /// matcher progress + profile dropped), a menu selection clamps near its old position, a
    /// preview/swap subject turns into its screen's own missing-route path. Changed summaries or
    /// identities dirty the map once. A replacing upload separately invalidates geometry-derived state.
    pub fn set_routes_with_ids(&mut self, summaries: &[RouteSummary], ids: &[crate::CatalogObjectId]) {
        let len = summaries.len().min(ids.len()).min(crate::MAX_ROUTES);
        if self.catalogs.routes() == &summaries[..len] && self.catalogs.route_ids() == &ids[..len] {
            return;
        }
        // The catalog + trip replacement (and the id ↔ summary pairing) is `CatalogState`'s; the
        // old-id snapshot it returns drives the remap of everything held *outside* it.
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
    /// Re-point every held catalog index after the catalog was replaced: old index → its id in
    /// `old_ids` → that id's new index (or `None` if the route vanished). See
    /// [`set_routes_with_ids`](App::set_routes_with_ids).
    fn remap_route_indices(&mut self, old_ids: &[crate::CatalogObjectId]) {
        let App { catalogs, ui, navigator, .. } = self;
        let remap = |i: usize| -> Option<usize> { catalogs.remap_route(old_ids, i) };

        // The active route, every route-derived cache, the seam request, and the undelivered detour
        // request follow the same durable identity in Navigator.
        navigator.remap_route_keys(&remap);
        navigator.remap_detour_route(&remap);
        navigator.remap_review_keys(&remap);

        // Every screen on the stack that holds a catalog index. The Route menu also takes the
        // re-resolved trips (`replace_routes` re-filed them before returning) + the new route count
        // so it can follow its highlight into the regrouped (folders + unfiled routes) list.
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

    /// Activate the route at catalog index `idx` (a host baseline / demo-reset seam) — the
    /// invariant-preserving twin of the menu's own selection, bounds-checked against the resident
    /// catalog, so hosts never write Navigator's active route directly to stage a route. An
    /// out-of-range index clears the active route. Dirties the map so an open Map repaints the line.
    pub fn activate_route(&mut self, idx: usize) {
        self.navigator.set_active_route((idx < self.catalogs.route_len()).then_some(idx));
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

    /// Replace the host's paired ride snapshot, newest first. Keep the
    /// newest [`UI_RIDES_CAP`](crate::UI_RIDES_CAP) summaries visible and retain expiry metadata for
    /// up to [`MAX_RIDES`](crate::MAX_RIDES) supplied rides. Re-point open screens by durable id
    /// across the rescan and dirty the map once.
    pub fn set_rides(&mut self, entries: &[RideEntry]) {
        let entries = &entries[..entries.len().min(crate::UI_RIDES_CAP)];
        if self.catalogs.rides() == entries {
            return;
        }
        // Screen indices follow the durable identity through each rescan.
        // Derived track answers already carry that identity and need no remap.
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

    /// The resident ride catalog (paired entries) — what the Rides screen lists.
    pub fn rides(&self) -> &[RideEntry] {
        self.catalogs.rides()
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
    ///   start between that reply and this drain),
    /// - the rider has confirmed a **shutdown** — the terminal powering-off frame is the last thing
    ///   the panel will hold, and a card over it would take
    ///   [`power_off_requested`](App::power_off_requested) back to `false` (#1515 D3). Deferring is
    ///   the honest answer: there is no next pass, and the request dies with the device rather than
    ///   cancelling a switch-off the rider asked for.
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
        // Through `apply`, not `stack.push`: this is the one card that does not come from the card
        // scheduler, and pushing raw would step around the rule every other arrival obeys —
        // *nothing lands on top of a drawer* (#1515 D3).
        screen::apply(
            &mut self.ui.stack,
            screen::Transition::Push(Screen::DfuCheck(crate::screen::DfuCheckScreen::new())),
        );
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
        // Raw, and deliberately: this is a bench line on the debug VCOM, not an arrival a rider can
        // produce, so it is not one of the sites the "nothing lands on top of a drawer" rule
        // (`screen::close_drawers`) exists for. Every rider-reachable push goes through
        // `screen::apply` or the card scheduler's `land`.
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

    /// Drop **everything derived from the active route's geometry** — the whole-App seam, and the
    /// only thing route-replacing paths should call.
    ///
    /// Navigator drops its matcher, caches, and visible route state. The UI also drops its
    /// next-category cache and corridor snapshot: their along-route distances belong to the old
    /// geometry even when the catalog index, filter, and frozen progress anchor stay unchanged.
    pub(crate) fn drop_route_derived_state(&mut self) {
        self.navigator.drop_route_derived_state();
        self.ui.next_ahead.invalidate();
        self.ui.corridor_scratch.invalidate();
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
                // The planner's own verdict is the one the rider is shown; the two resource
                // failures have no tier of their own and land on the generic card, which is what
                // the legacy protocol has always done with them.
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
                if self.navigator.released(&mut self.mode) || self.ui.find.state == crate::find_place::State::Releasing
                {
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
                let prev = self.navigator.replace_active_route(idx);
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
                self.navigator.set_active_route(Some(idx));
                self.navigator.request_seam(idx, anchor.unwrap_or(0));
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
        self.catalogs.set_detour_preview(pts, self.active_route_index());
        self.ui.map_dirty = true;
    }

    /// Whether a Route overview is up **without** its route-shape preview (#685 §4; #678 rework 3
    /// widened it from the computed overview to every overview — the stored-route page's track
    /// pager wants the shape too) — the host's per-pass cue to decimate the active route's
    /// polyline ([`RouteReader::preview_polyline`](obc_route::RouteReader::preview_polyline)) and
    /// hand it to [`set_nav_preview`](App::set_nav_preview). Entering the overview points
    /// Navigator's active route at the previewed route (the same key the
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
        let Some(key) = self.derived_needs().nav_preview else { return };
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
        if self.ui.stack.iter().any(|s| matches!(s, Screen::Journey(_))) || self.ui.find.resume_offer {
            self.ui.find.review = self.assistant_review_status();
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
        self.ui.hold_cancel_pending = true;
        self.ui.map_dirty = true;
    }

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
        let active_id = self.active_route_index().and_then(|i| self.catalogs.route_id_at(i));
        let active_replace = replaced && active_id == Some(id);
        if replaced {
            self.invalidate_current_visit(id);
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

    pub fn debug_stack_len(&self) -> usize {
        self.ui.stack.len()
    }

    /// Whether any drawer is on the stack — test/diagnostic observability for the "nothing lands on
    /// top of a drawer" rule ([`close_drawers`](crate::screen::close_drawers)).
    pub fn debug_stack_has_overlay(&self) -> bool {
        self.ui.stack.iter().any(|s| s.is_overlay())
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
    /// [`RideDamage::Catalog`](crate::RideDamage::Catalog) offers no action at all. Back cannot
    /// silently strand the object behind Home in either case.
    pub fn offer_damaged_ride(&mut self, damage: crate::RideDamage) -> bool {
        if !self.recorder.offer_recovery(crate::recorder::RideRecoveryState::for_damage(damage)) {
            return false;
        }
        self.recorder.restore_continuation(crate::RideContinuation::default());
        self.activity.mode = Mode::Idle;
        self.navigator.suspend_for_recording_recovery();
        self.raise_ride_recovery()
    }

    /// Root the UI at the recovery card in whatever mode Recorder's state names, and cancel any hold
    /// in flight so the card's guarded row starts from zero.
    ///
    /// **The one place the card is raised**, so the two boot offers and the pass's re-raise cannot
    /// drift. Re-rooting is also what repaints it: the card is `Static`-keyed, so a newly latched
    /// mode reaches the panel through the stack change rather than through a render key.
    /// `false` means the state names no decision to put.
    pub(crate) fn raise_ride_recovery(&mut self) -> bool {
        let Some(mode) = crate::screen::RecoveryMode::of(self.recorder.recovery()) else {
            return false;
        };
        screen::apply(
            &mut self.ui.stack,
            screen::Transition::Root(Screen::RideRecovery(crate::screen::RideRecoveryScreen::new(mode))),
        );
        self.ui.map_dirty = true;
        self.ui.input.cancel_holds();
        self.ui.hold_cancel_pending = true;
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
        match self.ui.stack.iter().rev().find(|screen| !screen.is_overlay()) {
            Some(Screen::PeakView(screen)) => Some(screen),
            _ => None,
        }
    }

    pub fn top_screen(&self) -> &Screen {
        self.ui.stack.last().expect("the stack always has the Home root")
    }

    /// Apply one device-wide [`Chord`]: a drawer toggle or direct Assistant entry.
    /// Returns whether it moved anything.
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
        // The **powering-off frame** refuses a squeeze, exactly as
        // [`escape_to_menu`](App::escape_to_menu) does — and it has to be said here rather than in
        // `Caps`, because the frame is a *page* of the quick drawer and a drawer must never declare
        // `blocks_chords` (that is the declaration the chord which closes it would trip over). The
        // rider has completed the guarded hold; a sheet opening over that frame would take
        // [`power_off_requested`](App::power_off_requested) back to `false` and cancel a shutdown
        // already in progress.
        if self.power_off_requested() {
            return false;
        }
        match chord {
            Chord::Quick => self.toggle_drawer(Screen::QuickDrawer(QuickDrawerScreen::opening())),
            Chord::Assistant => {
                if let Some(index) = self.ui.stack.iter().rposition(|s| matches!(s, Screen::Assistant(_))) {
                    self.ui.stack.truncate(index + 1);
                } else {
                    let assistant = Screen::Assistant(screen::AssistantScreen::new());
                    let transition = if self.ui.stack.len() < self.ui.stack.capacity() {
                        screen::Transition::Push(assistant)
                    } else {
                        screen::Transition::Root(assistant)
                    };
                    screen::apply(&mut self.ui.stack, transition);
                }
                self.ui.map_dirty = true;
                self.ui.last_input_ms = self.ui.now_ms;
                self.ui.idle_return_timing = true;
                self.ui.input.cancel_holds();
                self.ui.hold_cancel_pending = true;
                self.ui.reconcile_corridor(self.up_ahead_scope());
                true
            }
            // The contextual sheet exists only where content is declared (#1515 D3): a base screen
            // that names no [`ContextMenu`](crate::screen::ContextMenu) gets nothing, not an empty
            // drawer. The squeeze is still swallowed by the recogniser, so it can never leak a step
            // and a Back on the way to doing nothing.
            Chord::Context => match self.base_context() {
                Some(menu) => self.toggle_drawer(Screen::ContextDrawer(ContextDrawerScreen::opening(menu))),
                None => false,
            },
        }
    }

    /// What the Up-ahead timeline is scoped to right now: the rider's live category filter (app
    /// state, reset on entry) and their persisted source preference. One value, so the two halves
    /// of the scope can never reach the Assistant runtime apart.
    pub(crate) fn up_ahead_scope(&self) -> crate::corridor::UpAheadScope {
        crate::corridor::UpAheadScope { filter: self.state.up_ahead_filter, source: self.settings.up_ahead_source }
    }

    /// The [`ContextMenu`](crate::screen::ContextMenu) the **base** screen declares — the lowest
    /// non-overlay row, so a sheet already up does not hide the content the chord is asking about
    /// (which is what makes the same chord close the context drawer again).
    fn base_context(&self) -> Option<&'static crate::screen::ContextMenu> {
        self.ui.stack.iter().rev().find(|s| !s.is_overlay()).and_then(|s| {
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
            // The other drawer was on the panel a frame ago and the frozen base under it was never
            // redrawn, so the incoming sheet owes the draw that takes those rows off — the same
            // debt a shorter sheet swapped in for a taller one carries (#1559: covering is cheap,
            // uncovering is not).
            if closed.is_some() {
                if let Some(top) = self.ui.stack.last_mut() {
                    top.owe_base_draw();
                }
            }
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
        screen::powering_off(&self.ui.stack)
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
    /// key it wants through the Assistant runtime and
    /// [`reconcile_corridor`](crate::ui_runtime::UiRuntime::reconcile_corridor) re-points the scratch
    /// at it after every gesture and per-pass sweep — which is what disarms a request whose screen
    /// went away. So a request armed *here* survives only until the next reconcile unless some screen
    /// on the stack asks for the same key: this is the test/introspection door (and the pre-U3 seam),
    /// while a new consumer (U5's "Next: \<category\>" stat fields) adds its own `corridor_request`
    /// arm rather than calling this.
    pub fn arm_corridor(&mut self, filter: obc_reader::PoiCategorySet, anchor_m: u32) {
        self.ui.corridor_scratch.arm(crate::corridor::CorridorKey {
            hours_filter: obc_reader::reader::places::HoursFilter::HideClosed,
            filter,
            anchor_m,
        });
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

    /// The current **UTC** unix seconds, from the wall clock. The clock's set-point is local time
    /// (the UTC anchor shifted by the offset), so the persisted UTC offset is folded back out.
    pub fn wall_unix_now(&self) -> u32 {
        let local = self.wall_clock.unix_now(self.ui.now_ms);
        (local as i64 - self.settings.utc_offset_min as i64 * 60) as u32
    }

    /// The footer's wall-clock anchor for this pass: what the *device* knows about the time of day,
    /// which is not a ride accumulator and so is given to Recorder rather than derived by it. Stage
    /// 7 stamps it as it offers the operation slot, so the anchor an executor writes pairs with the
    /// samples that operation carries.
    pub(crate) fn footer_clock(&self) -> crate::recorder::FooterClock {
        crate::recorder::FooterClock {
            unix_at_anchor: self.wall_unix_now(),
            anchor_ms: self.ui.now_ms,
            trusted: self.clock_trusted(),
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
                self.navigator.route_state(),
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
    /// its input stage, from the same frame's clock. **Adopting it here instead is not free** — it
    /// was tried: `run_pass` brackets its before/after render-key comparison around every
    /// clock-driven change, so moving `now_ms` ahead of the *before* key hides one (the sensor
    /// staleness crossing, caught by `dirty_parity` at 14,000 ms). So a chord resolved here is
    /// applied against the *previous* pass's clock — which is why a drawer is not handed one: its
    /// open starts on the first frame that ticks it (#1569, `QuickDrawerScreen::opened_ms`).
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

    /// **The global escape** (#1515 D3): a completed Back-hold leaves whatever the rider is on and
    /// lands on the main [`Menu`](crate::screen::MenuScreen). Reports whether the stack moved.
    ///
    /// Three rules, and they are the whole of it:
    ///
    /// * **Any sheet goes with it.** A drawer is not a place to come back to — which is what makes
    ///   the escape work from a drawer subpage, including the power confirmation D2 could not
    ///   leave. It needs no step of its own: the general
    ///   [`close_drawers`](crate::screen::close_drawers) rule takes the sheet as the Menu lands.
    /// * **A decision the rider must answer keeps its answer.** The declared
    ///   [`Caps::blocks_escape`](crate::screen::Caps) set refuses the escape, as does the terminal
    ///   powering-off frame, whose own contract is that nothing dismisses it. Every *other* modal
    ///   — the planning spinner, the upload cards, a warning — lets the Menu open over it, which is
    ///   safe because a card that lands while the rider is away is written into its own stack slot
    ///   rather than pushed, so it never yanks the Menu and Back still finds it.
    /// * **It goes to the Menu; it never adds one.** With a Menu already on the stack the escape
    ///   *rewinds* to it instead of stacking a second, so escaping is idempotent at any depth —
    ///   not only when the Menu happens to be on top.
    ///
    /// That last rule is a bound, not a nicety. The escape is *the* gesture for "get me out of six
    /// levels of settings", so escape → re-descend → escape is its most ordinary use; pushing every
    /// time would let two laps of it reach [`MAX_DEPTH`](crate::screen::MAX_DEPTH), where the next
    /// host-pushed card is dropped. Rewinding makes the stack shrink on every lap after the first.
    ///
    /// The first escape **pushes**, so Back out of the Menu returns to the view the rider escaped
    /// from — the one thing the compass ride menu got right. Later escapes land on that same Menu,
    /// with the station the rider last used still selected.
    fn escape_to_menu(&mut self) -> bool {
        // Asked of the **base**, not of `stack.last()`: a sheet the rider opened over a card they
        // must answer is not consent to walk away from the card. A host-pushed blocking modal is
        // itself the base (it is no overlay), so it still answers for itself.
        let base = self.ui.stack.iter().rev().find(|s| !s.is_overlay());
        if base.is_some_and(|s| s.caps().blocks_escape) || self.power_off_requested() {
            return false;
        }
        // No explicit sheet-popping here: both arms below already take one. A rewind truncates to
        // the Menu, which is under every overlay; a push goes through
        // [`screen::apply`](crate::screen::apply), whose `Push` arm closes drawers because
        // *nothing lands on top of one*. A loop of its own was a third statement of that rule, and
        // a mutant proved it changed nothing.
        let mut changed = false;
        match self.ui.stack.iter().rposition(|s| matches!(s, Screen::Menu(_))) {
            // Rewind to the Menu the rider already has. Truncating to it is a no-op when it is
            // already on top, which is how a repeated squeeze of the bar costs nothing.
            Some(i) => {
                if i + 1 < self.ui.stack.len() {
                    self.ui.stack.truncate(i + 1);
                    changed = true;
                }
            }
            // No Menu anywhere: open one over what is there. `Root` is the fallback for the one
            // case a push cannot serve — a full stack with no Menu on it, which needs host cards
            // over a deep path to reach. Landing on `[Home, Menu]` loses the way back, and that is
            // still better than an escape that silently does not escape.
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
            self.ui.input.cancel_holds();
            self.ui.hold_cancel_pending = true;
        }
        changed
    }

    /// [`apply_gesture`](App::apply_gesture), reporting whether the transition **changed the screen
    /// stack** — the fact [`apply_gesture_batch`](App::apply_gesture_batch) needs to apply #480's
    /// drop rule without consuming the hold-cancel latch a second input plane still owns.
    fn apply_gesture_reporting_stack_change(&mut self, g: Gesture) -> bool {
        if g == Gesture::Press && self.activate_place_detail() {
            return true;
        }
        // Every screen renders into the map plane, so an applied gesture dirties it. Conservative by
        // design (a gesture a screen ignores still costs one redraw), which keeps the idle path
        // exact: with no gesture recognized, `apply_gesture` never runs and the map stays clean.
        self.ui.map_dirty = true;
        // Any recognised gesture is user activity: reset the idle-return clock (see
        // `apply_idle_return`). A gesture the screen ignores still counts — a step on Home, say.
        self.ui.last_input_ms = self.ui.now_ms;
        self.ui.idle_return_timing = true;
        // **The global escape** (#1515 D3): Back-hold reaches the main menu from anywhere, so it is
        // resolved here, above screen dispatch, and no screen binds it any more.
        if g == Gesture::Press {
            if let Some(Screen::Assistant(screen)) = self.ui.stack.last() {
                let selected = screen.selected;
                let before = self.ui.stack.len();
                match selected {
                    0 => self.open_find_place(),
                    1 => self.open_whats_next(),
                    2 => {
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
                    5 => self.open_landmarks(),
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
        // Snapshot the settings so a settings-screen edit is detected by one `==` (Settings is
        // `Copy + Eq`). A change flags a save for the host to pick up via `take_settings_dirty`.
        let settings_before = self.settings;
        let place_local = self.place_local_time();
        // Navigator's detour level before the screen speaks, so a cancellation it admits takes the
        // preview polyline with it (see `sync_detour_preview`).
        let detour_planned_before = self.navigator.detour_planned();
        let backlight_available = self.backlight_available;
        let App { state, activity, settings, catalogs, nav_profiles, recorder, ui, navigator, dfu, storage, .. } = self;
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
            nav_profiles,
            backlight: backlight_available,
            poi_scratch: &ui.poi_scratch,
            corridor: ui.corridor_scratch.entries(),
            sensor_scan_hits: ui.sensor_scan_hits.as_slice(),
            now_ms: ui.now_ms,
        };
        let mut t = ui.stack.last_mut().expect("the stack always has the Home root").handle(g, &mut cx);
        let depth_before = ui.stack.len();
        // Whether this transition actually changes the stack (Pop/Home at the root are no-ops).
        // A change invalidates any in-flight hold's target — see `hold_cancel_pending`.
        let stack_changed = match &t {
            screen::Transition::None => false,
            screen::Transition::Pop | screen::Transition::Home => depth_before > 1,
            screen::Transition::Push(_) | screen::Transition::Replace(_) | screen::Transition::Root(_) => true,
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
        let scope = self.up_ahead_scope();
        self.ui.reconcile_corridor(scope);
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
        self.advance_easier();
        let now = self.wall_clock.now(clock.0);
        let ms_to_next_minute = self.wall_clock.ms_to_next_minute(clock.0);
        let pan_active = self.state.pan.is_some();
        let tracking = self.recorder.recording();
        // The timer poll itself — and every stack/dirty/wake mutation it makes — is the UI
        // runtime's; this method sequences the per-pass sweeps around it with the cross-component
        // facts they need.
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
        let scope = self.up_ahead_scope();
        let navigation = self.navigator.route_state();
        self.ui.reconcile_next_ahead(&self.settings, scope, navigation.active_route, navigation.progress_m);
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
        self.render_scene_map_timed(
            scratch,
            target,
            Some(reader),
            Some(reader),
            route,
            None,
            w,
            h,
            color_fn,
            &NoopClock,
        )
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
        self.render_scene_map_timed(scratch, target, reader, reader, route, None, w, h, color_fn, clock)
    }

    /// Generic timed map-plane render. `scene` drives geometry through [`MapScene`];
    /// `core_reader` drives the core-only POI/hours preparation. They are independently optional
    /// so chrome-only frames can skip every map source.
    #[allow(clippy::too_many_arguments)]
    pub fn render_scene_map_timed<D, F, S>(
        &mut self,
        scratch: Option<&mut RenderScratch>,
        target: &mut D,
        scene: Option<&S>,
        core_reader: Option<&Reader>,
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
        S: MapScene,
    {
        self.render_scene_map_photo_timed(
            scratch,
            target,
            scene,
            core_reader,
            route,
            peak_view,
            w,
            h,
            color_fn,
            clock,
            None,
        )
    }

    /// Render the base, prepare bounded photo work, then compose covering screens.
    #[allow(clippy::too_many_arguments)]
    pub fn render_scene_map_photo_timed<D, F, S>(
        &mut self,
        scratch: Option<&mut RenderScratch>,
        target: &mut D,
        scene: Option<&S>,
        core_reader: Option<&Reader>,
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
        S: MapScene,
    {
        // Record the panel size for the screen ticks' region reporting (`advance_animations`) —
        // the one place every host states its real frame dimensions.
        self.ui.frame_size = (w as i16, h as i16);
        self.prepare_find(core_reader, route);
        self.prepare_peak_article(core_reader);
        self.prepare_landmarks(core_reader);
        // Route-relative pan steps are recorded by gesture handling as a cumulative-distance
        // cursor because `Ctx` deliberately owns no streamed reader. Resolve that cursor here,
        // once per dirty step, before `Render` borrows state read-only for the draw pass.
        if let Some(route) = route {
            self.state.sync_pan_route(route);
        }
        if scratch.is_some() && self.ui.base_draws_map() && self.ui.render_clip.is_none() {
            self.ui.map_icons.prepare(core_reader, &self.state.viewport(w, h), &self.settings, self.ui.now_ms);
            // The overlay has no rider switch yet; `true` is the input a switch would drive.
            self.ui.settlements.prepare(core_reader, &self.state.viewport(w, h), true);
        }
        // Drain the one-shot region clip (see `set_render_clip`) — `None` on every normal frame.
        let render_clip = self.ui.render_clip.take();

        // Rebuild the cached elevation profile when the active route changes — it streams every
        // chunk, so it's built once on load, never per frame; clears when no route is loaded.
        self.navigator.refresh_route_profile(route);
        if self.ui.stack.iter().any(|s| matches!(s, Screen::WhatsNext(_))) {
            let scope = self.up_ahead_scope();
            let local = self.place_local_time();
            self.ui.ahead.prepare(
                core_reader,
                route,
                self.navigator.climbs(),
                scope,
                &mut self.ui.corridor_scratch,
                local,
            );
            if self.ui.ahead.pending() {
                self.ui.map_dirty = true;
                self.ui.next_wake_ms = Some(1);
            }
        }
        // Invalidate the resident **ride** profile + track preview the moment they stop matching
        // the viewed ride (#680; the preview joined in #678 rework 3): the detail exited
        // (`viewed_ride` cleared) or moved subjects. Filling is the executor's keyed answer; only
        // the drop lives here, so a stale band/shape is never drawn.
        let key = self.catalogs.ride_track_key(self.activity.viewed_ride);
        self.catalogs.drop_stale_ride_views(key);

        // Pre-draw acquisition (#803): the base screen resolves any streamed-reader state (POI
        // snapshot / hours or Detour route geometry) before the draw loop, so `Render` carries
        // the POI scratch read-only and every screen's `draw` is side-effect-free.
        let navigation = self.navigator.route_state();
        self.ui.prepare_base(
            core_reader,
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
        let base = self.ui.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);

        // The in-screen confirm fill's hold-progress. Prefer a host-supplied value (the two-plane
        // firmware's separate input plane); fall back to `App`'s own input on the single-loop hosts.
        let hold_progress = self.ui.hold_progress_override.unwrap_or_else(|| self.ui.input.select_hold_progress());
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
            nav_profiles,
            fw_version,
            map_name,
            map_obcm_version,
            storage,
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
        // Bundle the active climb for the screens: the resident detail buffer is only meaningful
        // when a climb is active, so hand out the `(seg, profile)` pair exactly when `active_climb`
        // resolves to a live segment — a stale buffer is never reachable through `Render`.
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
            activity,
            navigation,
            recorder,
            settings,
            routes: catalogs.routes(),
            unaccepted_routes: navigator.unaccepted_routes(),
            internal_routes: navigator.internal_routes(),
            rides: catalogs.rides(),
            trips: catalogs.trips(),
            nav_profiles,
            route,
            profile: navigator.profile(),
            ride_profile: catalogs.ride_profile_for(ride_key),
            climb,
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
        let mut rx = RenderFrame { scene, render: rx };
        // The one Canvas of the frame: every screen draws through it (the base screen — the only
        // possible Map — writes `rx.stats`; the overlays above it leave the stats untouched).
        // A drained region clip makes it reject whole out-of-region primitives — the half of a
        // region-scoped repaint the target's pixel clip can't save (#500 follow-up).
        // A drawer **recesses** the base rather than replacing it: the base draws through the dim
        // LUT composed with the host's own colour policy, the sheet through the untouched one. No
        // capture buffer, no second framebuffer, no alpha for a 64-colour panel to approximate.
        // Whether it recesses at all is the *base's* declaration — `Caps::recess`, false for a map
        // base, whose second draw is a map render (#1559).
        //
        // The switch is a `Cell` inside **one** colour closure rather than a second `Canvas` with a
        // second closure type, and that is not a style choice: `Screen::draw` is generic over the
        // colour function, so a second closure type monomorphises the *entire* screen catalogue and
        // the map renderer a second time — measured at +147 KB of flash on the board. One closure
        // type, and one load-and-branch per **colour resolution** — `Canvas` resolves `color_fn`
        // once per primitive (a span, an outline, a string), so this is O(primitives), not
        // O(pixels).
        let covered = ui.base_frozen();
        let recessed = covered && ui.stack.get(base).is_none_or(|s| s.caps().recess);
        let recess = core::cell::Cell::new(recessed);
        let policy = |c: u16| color_fn(if recess.get() { screen::dim_color(c) } else { c });
        // **The frozen base's pixels** (#1559) — the frozen base, finished. A drawer's render key
        // already shadows the base's, so no fact the base draws can move while a sheet is up: its
        // rows on the panel are right, and drawing them again only puts back what is already there.
        // On a **resident** target the base's draw is therefore skipped altogether, and an open
        // step costs the sheet and nothing else — the map render the open used to pay for at each
        // step (199 ms at the riding default, 1.45 s at 5 m/px, measured) is not paid at all. The
        // three exclusions, and why each is one, are on [`UiRuntime::sheet_only`] — which the
        // frame's `Reader` need reads too, because a frame that skips this draw reads nothing.
        let preserve_photo = photo.as_ref().is_some_and(|work| !work.redraw)
            && ui.resident_frame
            && matches!(ui.stack.get(base), Some(Screen::LandmarkPhoto(_)));
        let sheet_only = ui.sheet_only() || preserve_photo;
        // The one Canvas of the frame: every screen draws through it (the base screen — the only
        // possible Map — writes `rx.stats`; the overlays above it leave the stats untouched).
        // A drained region clip makes it reject whole out-of-region primitives — the half of a
        // region-scoped repaint the target's pixel clip can't save (#500 follow-up).
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
                            work.runtime.step(page, core_reader, target, color, rx.settings.language);
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
        // The one name this frame asked to scroll. A fresh name's first step is armed here, after
        // the draw that named it — the pass's own wake was planned before the render, so the
        // firmware reads the deadline again once the frame is drawn.
        if let Some(wake) = ui.marquee.adopt(marquee, ui.now_ms) {
            ui.next_wake_ms = Some(ui.next_wake_ms.map_or(wake, |w| w.min(wake)));
        }
        // **The frame pays what the sheets above the base owed** (#1515 D5). A sheet arms a base
        // draw when it stops purely covering the screen below — a page slide, a shorter sheet
        // swapped in — and carries it until a frame draws that screen. This is that frame, and
        // `!sheet_only` is the only place the answer exists: a pass may tick and then render
        // nothing at all.
        if !sheet_only {
            ui.spend_base_draw();
        }
        stats
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
    /// the freeze is not engaged — the twin of [`InputPlane::overlay_rows`](crate::InputPlane::overlay_rows)
    /// for a partial-overlay host (the board re-presents overlay *rows*, not whole frames). A host
    /// that pushes the union of this and the bulge's rows presents exactly what changed.
    pub fn reroute_banner_rows(&self, h: f32) -> Option<(u16, u16)> {
        self.planning_banner().map(|_| crate::screen::vocab::chrome::recalculating_banner_rows(h))
    }

    /// Whether the overlay plane has live content this frame — a hold bulge charging, popping, or
    /// retracting. `false` exactly when [`render_overlay`](App::render_overlay) would draw nothing,
    /// so a host driving the overlay as a separate layer can leave it idle.
    pub fn overlay_active(&self) -> bool {
        self.ui.input.overlay_active() || self.planning_banner().is_some()
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
    /// The overlay plane is **derived here, from levels** — the hold bulge's and the planning
    /// freeze's, read as one [`OverlayKey`](crate::device_core::pass::OverlayKey) and folded against
    /// the level this same call last saw. Both rules live in that one converter: see its doc for why
    /// the banner keys on the engaged level rather than on the plan's own start edge.
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
    /// sim's snapshot path) never call this.
    pub fn set_render_clip(&mut self, clip: Option<Rectangle>) {
        self.ui.render_clip = clip;
    }

    /// Declare that this host's render target **keeps the last frame** between renders — the
    /// resident panel plane a board or a windowed host draws into, as against a buffer composed
    /// from nothing every time (the snapshot sweep, a one-shot capture).
    ///
    /// A resident target is what makes a repaint able to be *partial*, and it is the precondition
    /// for the frozen base's pixels (#1559): with a drawer over the base, the base's draw is
    /// skipped and its rows simply stand, so an open step costs the sheet and no map render. A host
    /// that says nothing gets every screen drawn every time, which is always correct.
    ///
    /// Declared once at composition, like [`set_backlight_available`](App::set_backlight_available)
    /// — it is a property of the host's plumbing, not of any frame.
    ///
    /// **A host that claims this untruthfully is not caught by anything.** The differential replay's
    /// power comes from its reference *not* declaring it, so a host asserting a residency it does
    /// not have simply silences the oracle. Read the host's frame path before adding a fourth
    /// caller: the target must be one buffer that survives between renders and is never cleared.
    pub fn set_resident_frame(&mut self, resident: bool) {
        self.ui.resident_frame = resident;
    }

    /// The current operating mode.
    pub fn mode(&self) -> Mode {
        self.activity.mode
    }
}

impl App {
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
        let assistant =
            self.ui.stack.iter().rev().find(|s| !s.is_overlay()).is_some_and(
                |s| matches!(s, Screen::VisitReview(s) if s.accepted && self.current_visit_index().is_some()),
            );
        let overview_open = assistant || self.ui.stack.iter().any(|s| matches!(s, Screen::RouteOverview(_)));
        let nav_preview = overview_open
            .then(|| self.catalogs.nav_preview_key(self.active_route_index(), assistant))
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
            let effect = app.settings_ops.next_effect(in_subtree, now_ms)?;
            let crate::settings::SettingsEffect::PersistRevision { token, revision } = effect;
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

    #[test]
    fn assistant_shortcut_at_full_stack_leaves_room_for_its_actions() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        while app.ui.stack.len() < app.ui.stack.capacity() {
            screen::apply(&mut app.ui.stack, screen::Transition::Push(Screen::Menu(MenuScreen::new())));
        }
        assert!(app.apply_chord(Chord::Assistant));
        assert!(matches!(app.top_screen(), Screen::Assistant(_)));
        app.apply_gesture(Gesture::Press);
        assert!(matches!(app.top_screen(), Screen::FindPlace(_)));
        app.apply_gesture(Gesture::Back);
        assert!(matches!(app.top_screen(), Screen::Assistant(_)));
        app.apply_gesture(Gesture::Back);
        assert!(matches!(app.top_screen(), Screen::Home(_)));
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

    /// **A sheet-only frame asks the host for no `Reader`** (#1569). The base's draw is skipped, so
    /// the SD style-table parse behind it would be a parse the frame throws away — and on the board
    /// that parse is about half the cost of an open step.
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

    /// The two drawers are mutually exclusive on the stack, and the panel has to agree: the sheet
    /// that replaces the other arrives over rows the departed sheet still holds (the frozen base
    /// was never redrawn under it), so its first frame draws the base — and only its first.
    /// Without this the board kept the quick drawer's ink at the top while the context sheet slid
    /// up from the bottom.
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

    /// Starting a ride with no fix yet arms the session immediately (Riding, banner up) but stages
    /// nothing and books no moving time — then the first fix stages the segment anchor and clears
    /// the banner ("start before lock").
    #[test]
    fn tracking_arms_without_a_fix_and_stages_on_first_fix() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.test_start_ride(); // a route load arms a tracking session

        // A tick with no fix: armed, but nothing staged and no moving time accrued.
        let mut loc = OneFix(None);
        app.ui.now_ms = 1_000;
        app.tick(RideClock(1_000), Sensors::new(&mut loc), None);
        assert!(app.recording(), "the session is armed immediately, fix or not");
        assert!(!app.has_live_fix(1_000), "no fix yet → the banner is up");
        assert!(app.recorder.staged().is_empty(), "nothing recorded while searching");
        assert_eq!(app.recorder.moving_s(), 0.0, "moving time idles until the first fix");

        // The first fix lands → it is staged (the segment anchor) and the banner clears.
        let mut loc = OneFix(Some(Fix::at(0, 0)));
        app.ui.now_ms = 2_000;
        app.tick(RideClock(2_000), Sensors::new(&mut loc), None);
        assert!(app.has_live_fix(2_000), "the fix landed → banner clears");
        assert_eq!(app.recorder.staged().len(), 1, "the first fix stages the segment anchor");
        assert!(app.recorder.staged()[0].segment_start, "…as a segment anchor");
    }

    /// A ride log that loses a point must not lose it silently: the staging buffer is bounded, and a
    /// ride whose samples no executor ever writes fills it and raises the dismissable
    /// recording-error card — the core of issue #11, now on the app's own half of the seam. Latched
    /// once per boot, so a whole ride of lost points is one card.
    #[test]
    fn a_ride_log_that_loses_a_sample_raises_the_recording_error_warning() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.test_start_ride();
        assert!(!app.ui.stack.iter().any(|s| matches!(s, Screen::Warning(_))), "a healthy ride shows no warning card");

        // Nothing serves the append, so the staged samples pile up. ~11 m per second: every fix is
        // logged and none is a teleport.
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

        // Dismiss it; the fixes that keep failing don't nag again.
        app.apply_gesture(Gesture::Back);
        let mut loc = OneFix(Some(Fix::at(0, 100 * 40)));
        app.ui.now_ms = 40_000;
        app.tick(RideClock(40_000), Sensors::new(&mut loc), None);
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
        assert_eq!(app.recorder.climb_m(), 0.0, "the first sample only anchors");
        tick_alt(&mut app, 102.0, 2000); // +2 m: inside the dead-band
        assert_eq!(app.recorder.climb_m(), 0.0, "sub-dead-band noise books nothing through tick");
        tick_alt(&mut app, 110.0, 3000); // +10 m from the 100 m reference
        assert_eq!(app.recorder.climb_m(), 10.0, "a clean climb books through the full tick path");
    }

    /// The pause rule end-to-end: with the activity paused, `tick` still records the latest altitude
    /// but must not book climb across the rest, so barometer drift while stopped doesn't inflate
    /// `climbed` on resume. Proves the whole tick path honours the mode gate.
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
        app.test_start_ride(); // a ride, so the staged samples show what was actually recorded
        let mut terrain = FlatTerrain { height_m: 1800, samples: 0 };
        // Before any terrain sample the tile is the plain barometric reading, as it always was.
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
        assert!(!app.recorder.altitude().settled(), "no residual ever arrived");
        assert_eq!(app.recorder.altitude().offset_m(), None);
        assert_eq!(app.recorder.current_elevation_m(), app.recorder.baro_elevation_m());
        assert_eq!(app.recorder.current_elevation_m(), Some(699.0));
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
    /// *before* `record_fix`, that same tick's interval must book them — if the drain order ever
    /// regressed to after the fix, the summary accessors would read `None` here.
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

        // Live: each poll landed in Activity, timestamped at this tick.
        assert_eq!(app.recorder.live_hr(2_000), Some(150), "tick drained the HR strap");
        assert_eq!(app.recorder.live_power(2_000), Some(250), "tick drained the power meter");
        assert_eq!(app.recorder.live_cadence(2_000), Some(90), "tick drained the cadence sensor");
        // Summaries: the same-tick samples were booked into the same tick's moving interval —
        // proving the drains run before `record_fix` (else `hr_ms` would still be 0 → `None`).
        assert_eq!(app.recorder.avg_hr(), Some(150), "the moving interval booked the fresh HR");
        assert_eq!(app.recorder.max_hr(), Some(150));
        assert_eq!(app.recorder.avg_power(), Some(250), "…and the fresh power");
        assert_eq!(app.recorder.max_power(), Some(250));
        assert_eq!(app.recorder.avg_cadence(), Some(90), "…and the fresh cadence");
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
        assert_eq!(app.recorder.live_hr_display(), Some(142), "the tile shows the value across the divergence");
        // The old, wrong path — reading against the render clock — is what blanked the tile.
        assert_eq!(
            app.recorder.live_hr(app.ui.now_ms),
            None,
            "the render-clock read is stale (90 s vs a 30 s sample) — the bug `_display` fixes"
        );

        // And staleness still works on the ride clock: advance the tick clock 6 s past the sample
        // with no new reading → the tile blanks, exactly as a dropped strap should.
        app.recorder.note_sensor_clock(36_001);
        assert_eq!(app.recorder.live_hr_display(), None, "a >5 s-old sample still blanks — no frozen value");
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
            RideScreen, SettingsScreen, StatFieldsScreen, SystemScreen, Transition, UnitsScreen,
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
        let cases: [Case; 11] = [
            // Pure navigation — no edit gesture of its own.
            ("Settings list", || one(Screen::Settings(SettingsScreen::new())), &[]),
            // Open the UTC-offset stepper (#641: the one editable row), +one step — and leave the
            // field open, so Back must still close it then exit.
            ("Date & Time", || one(Screen::DateTime(DateTimeScreen::new())), &[Gesture::Press, Gesture::Step(1)]),
            // Press flips metric ↔ imperial.
            ("Units", || one(Screen::Units(UnitsScreen::new())), &[Gesture::Press]),
            // → the Page-cycle row (index 1), open its stepper, +1 s (and leave it open — Back must
            // still close it then exit).
            ("Ride", || one(Screen::Ride(RideScreen::new())), &[Gesture::Step(1), Gesture::Press, Gesture::Step(1)]),
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

    /// The deepest ordinary mid-ride settings path leaves room for the host's cards. Walk the real
    /// navigation with gestures, then prove a host-pushed warning can still land instead of being
    /// silently dropped. The path lost a slot with the compass ride menu (#1515 D3): the global
    /// escape lands the Menu directly on the riding view instead of on a screen in between.
    ///
    /// **This walks the deepest *modelled* path; the reserve is only real if it is also the deepest
    /// *reachable* one.** For a while it was not: the escape pushed a Menu unconditionally, so
    /// laps of escape → re-descend grew past this without ever coming through here. That bound is
    /// `laps_of_escape_and_re_descent_leave_room_for_a_host_card`, and the two are read together.
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
        assert_eq!(crate::screen::MAX_DEPTH - app.ui.stack.len(), 3, "three host-card slots stay reserved");

        app.on_warning(WarningFlags::REC_ERROR);
        assert_eq!(app.ui.stack.len(), 8, "the host warning pushes over the deepest normal path");
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

    /// A **screen** always outranks the stat-field cache for the one shared corridor scratch: opening
    /// the Up-ahead list re-points it at the list's own key, and the cache's request waits (its
    /// harvest only ever accepts its own key, so the list's snapshot can't land in a tile).
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

    /// `ms_until_next_wake` reports the soonest timed-redraw deadline across the visible stack. On
    /// Home it's the wall-clock minute boundary; on a static menu the idle-return timeout is the
    /// only pending wake (the menu itself animates on nothing). With the idle return disabled a
    /// static menu reports `None` — sleep until input.
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
        for state in [State::Querying, State::Planning, State::Releasing] {
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

    // --- climb state tracking (C3, #509) ---
    //
    // The **pure** hysteresis resolvers (`resolve_active_climb` / `resolve_next_waypoint`) are
    // pinned in `navigator/following.rs`, next to the policy they encode. Here the App-side wiring is
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

    /// Pin the composed route-following result through the App tick. The route lives only in RAM
    /// and carries the inputs that exercise each guidance layer together: elevation for one climb,
    /// named waypoints, an off-route excursion, and a final fix at the route end.
    #[test]
    fn composed_guidance_trace_is_stable() {
        use obc_formats::io::{ByteSink, Error};

        #[derive(Default)]
        struct VecSink(std::vec::Vec<u8>);
        impl ByteSink for VecSink {
            fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
                self.0.extend_from_slice(bytes);
                Ok(())
            }

            fn patch_at(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Error> {
                let offset = offset as usize;
                self.0[offset..offset + bytes.len()].copy_from_slice(bytes);
                Ok(())
            }
        }

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

    /// Drive the active-climb state directly through `App::update_active_climb` with a controlled
    /// `progress_m`, over the real fixture reader — isolating the hysteresis + once-per-entry refill
    /// from the matcher's fix-snapping (which can't place progress to the metre).
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
        // Exactly one refill per climb *entry* — never per fix on the same climb. Three climbs, and
        // because they're back-to-back the sweep enters all three: 3 entries ⇒ 3 fills.
        assert_eq!(entries, 3, "the sweep enters each of the 3 climbs once");
        assert_eq!(
            app.navigator.climb_fill_count(),
            3,
            "the detail buffer is rebuilt exactly on the 3 entries, not per fix"
        );
    }

    /// Off-route freezes the active climb: a stale (frozen) match must not strand the rider onto a
    /// climb, nor drop the one they were on — the state holds until they rejoin and progress moves.
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
        app.navigator.route_state_mut().active_route = Some(0);
        app.navigator.route_state_mut().progress_m = 1_000;
        app.navigator.route_state_mut().route_total_m = 20_000;
        app.test_start_ride();

        // Plan a detour and land its preview, exactly as the flow does.
        let chooser = DetourScreen::new(app.navigator.route_state());
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
        app.navigator.route_state_mut().progress_m = 50_000;
        app.update_active_climb(&route);
        assert_eq!(app.navigator.route_state_mut().active_climb, None, "past every climb → no active climb");
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
        app.navigator.route_state_mut().progress_m = 50_000;
        app.update_active_climb(&route);
        assert_eq!(app.navigator.route_state_mut().active_climb, None);
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

        // Load the route (active_route = Some) and tick with the reader → climbs segmented once, and
        // the waypoint table loaded on the same edge (GRIMSEL carries none, so the table is empty but
        // the build key advances to Some(0) — the load ran).
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

        // Unload (active_route → None) and tick → the climbs / waypoints and their derived indices clear.
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
        app.set_rides(&[
            crate::RideEntry { id: 7, summary: ride("A") },
            crate::RideEntry { id: 9, summary: ride("B") },
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
        app.set_rides(&[crate::RideEntry { id: 9, summary: ride("B") }]);
        assert_eq!(app.activity.viewed_ride, Some(0), "the viewed index follows the id");
        assert_eq!(ride_track_request(&app), None, "the answer moved with it");

        // The viewed ride itself vanishing clears the keys — nothing left to request.
        app.set_rides(&[crate::RideEntry { id: 7, summary: ride("A") }]);
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

    // ==================== keyed derived data (#1437) ====================
    //
    // The four rules the epic locks for a level-triggered read, exercised through the real seam:
    // a need repeats until answered, a failure answers it, an input for a key that is no longer
    // current changes nothing, and changing the subject creates a new key.

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
        let trips = [crate::trip::TripInput { id: 20, name: "Tour", stage_ids: &[10] }];
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

    /// The nav-preview twin of the staleness rule, over the one thing identity cannot catch: an
    /// upload that replaces a stored route keeps the identity and changes the geometry.
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
