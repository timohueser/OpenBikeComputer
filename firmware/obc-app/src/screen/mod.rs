//! The screen system — `no_std`, zero-alloc, no retained widget tree. Screens are a
//! [`Screen`] enum dispatched by `match` (static dispatch), each variant a small module
//! with typed state. Navigation is a return value: [`handle`](Screen::handle) returns a
//! [`Transition`] that [`apply`] runs against a [`heapless::Vec`] stack.
//!
//! The shared context is split by role: [`Ctx`] is the logic half handed to `handle`
//! (mutable camera/mode + clock), [`Render`] is the draw half (read-only state plus the
//! `Reader`, the host's borrowed `RenderScratch`, and the in-flight hold-progress for the confirm
//! ring).
//!
//! This module holds the navigation engine only — the contexts, [`Transition`], [`Caps`], the
//! `screens!` table and the ride-session entry points. The drawing vocabulary every screen composes
//! its page from lives one module per concept under [`vocab`].

use core::ops::{Deref, DerefMut};

use embedded_graphics::{draw_target::DrawTarget, primitives::Rectangle};
use obc_map_scene::MapScene;
use obc_ports::Fix;
use obc_reader::Reader;
use obc_render::{Canvas, Clock, RenderScratch, RenderStats};
use obc_route::{ClimbProfile, ClimbSeg, Profile, RouteReader, Waypoints};

use crate::activity::{Activity, Mode};
use crate::app::AppState;
use crate::breadcrumb::Breadcrumb;
use crate::input::Gesture;
use crate::ride::RideSummary;
use crate::route::RouteSummary;
use crate::settings::{DateTime, Settings};

mod climb;
pub(crate) mod context_drawer;
mod detour;
mod dfu;
mod home;
mod map;
mod map_transfer;
mod menu;
mod nav_route;
mod passkey;
mod poi_detail;
mod poi_list;
pub(crate) mod poi_menu;
mod quick_drawer;
mod ride_control;
mod ride_detail;
mod ride_recovery;
mod ride_start;
mod rides;
mod route_menu;
mod route_overview;
mod route_received;
mod route_swap;
mod settings;
mod statistics;
mod trip_delete;
pub(crate) mod up_ahead;
pub(crate) mod vocab;
mod warning;
mod weather_alert;
mod weather_dash;
mod weather_hourly;
pub mod weather_icons;
mod weather_map;

pub use climb::ClimbScreen;
pub(crate) use context_drawer::ContextFacts;
pub use context_drawer::{ContextDrawerScreen, ContextMenu, ContextValue};
pub use detour::{DetourPreviewScreen, DetourScreen};
pub use dfu::{
    DfuCheckScreen, DfuConfirmScreen, DfuErrorReason, DfuErrorScreen, DfuFailedScreen, DfuInstallingScreen,
    DfuProgressScreen, DfuUpdatedScreen,
};
pub use home::HomeScreen;
pub(crate) use map::low_battery_cue;
pub use map::{MapScreen, ROUTE_WEIGHT};
pub use map_transfer::{MapTransfer, MapTransferError, MapTransferScreen};
pub use menu::MenuScreen;
pub use nav_route::{NavConfirmScreen, NavFailScreen, NavPlanningScreen, PlanKind};
pub use passkey::PasskeyScreen;
pub use poi_detail::PoiDetailScreen;
pub use poi_list::{PoiListScreen, PoiScratch};
pub use poi_menu::PoiMenuScreen;
pub use quick_drawer::{QuickDrawerScreen, BRIGHTNESS_LEVELS, BRIGHTNESS_MAX};
pub use ride_control::RideControl;
pub use ride_detail::RideDetailScreen;
pub use ride_recovery::RideRecoveryScreen;
pub use ride_start::RideStartScreen;
pub use rides::RidesScreen;
pub use route_menu::RouteMenuScreen;
pub use route_overview::RouteOverviewScreen;
pub use route_received::{RouteReceivedScreen, RouteUpdatedScreen, TripReceivedScreen};
pub use route_swap::RouteSwapScreen;
pub use settings::{
    AboutScreen, AddFieldScreen, BikeTypeScreen, BluetoothScreen, ConnectionsScreen, DateTimeScreen, DisplayScreen,
    FirmwareScreen, LanguageScreen, PowerScreen, ResetScreen, RideScreen, SensorScanScreen, SensorsScreen,
    SettingsScreen, StatFieldsScreen, SystemScreen, UnitsScreen, WeatherSettingsScreen,
};
pub use statistics::StatisticsScreen;
pub use trip_delete::TripDeleteScreen;
pub(crate) use up_ahead::poi_row_name;
pub use up_ahead::{UpAheadScreen, OFF_ROUTE_HINT_M};
/// The one exception to the vocabulary's import rule: the wait spinner's dirty disc is part of the
/// host-facing repaint contract (`ScreenTick::region`), so it is re-exported for the integration
/// tests that pin it. In-crate callers still import `vocab::spinner`.
pub use vocab::spinner::needle_region;
pub use warning::{WarningFlags, WarningScreen};
pub use weather_alert::{WeatherAlertKind, WeatherAlertScreen};
pub use weather_dash::WeatherScreen;
pub use weather_hourly::WeatherHourlyScreen;
pub use weather_map::WeatherRainMapScreen;

/// Maximum overlay depth. The deepest normal path is seven screens
/// (`Home → Map → Menu → Settings → Ride → Fields → Add field`); keep the rest of the slots
/// for host-pushed cards such as a warning arriving while that path is open.
pub const MAX_DEPTH: usize = 10;

/// The screen stack: the bottom is the always-present root (Home), the top is the
/// screen currently receiving input.
pub type Stack = heapless::Vec<Screen, MAX_DEPTH>;

/// What a screen's [`handle`](Screen::handle) asks the navigation stack to do next; [`apply`] runs it.
pub enum Transition {
    /// Stay on this screen (the gesture was handled in place, or is unbound).
    None,
    /// Open `screen` as the new top — a forward navigation or an overlay.
    Push(Screen),
    /// Return to the screen that opened this one — the `back` / Resume escape.
    Pop,
    /// Swap this screen for `screen` without growing the stack — sibling moves
    /// (Map ↔ Elevation) and "consume this screen" steps (Route menu → Map).
    Replace(Screen),
    /// Truncate to the Home root and push `screen`, landing on a clean `[Home, screen]` from any
    /// depth rather than leaving stale Menu / Route-menu screens buried under the new Map.
    Root(Screen),
    /// Clear every overlay back to the Home root — Finish / Discard / power-down.
    Home,
}

/// Whether the device is on the terminal **powering-off** frame: the rider completed the guarded
/// hold and the host is about to call the power-off port, so this is the last thing the panel will
/// hold. Nothing dismisses it.
///
/// A *stack* fact rather than an `App` one, because the operations that must respect it are stack
/// operations — [`close_drawers`] and the card scheduler's own `land`, neither of which has an
/// `App`. [`App::power_off_requested`](crate::App::power_off_requested) reads it too, so there is
/// one definition of "already switching off".
pub(crate) fn powering_off(stack: &Stack) -> bool {
    matches!(stack.last(), Some(Screen::QuickDrawer(d)) if d.powering_off())
}

/// **Nothing lands on top of a drawer** (#1515 D3): take any open sheet off the top of `stack`, and
/// report whether one was there. Called wherever an ordinary screen arrives — [`apply`]'s `Push`
/// arm and the card scheduler's own `land`, which pushes directly.
///
/// A drawer is transient chrome. A host card burying one would strand it: dismissing the card would
/// drop the rider back into a sheet they had finished with, and the global escape's "any sheet goes
/// with it" rule would hold only for the top slot. This is that rule made total, in the one place
/// each arrival passes through.
pub(crate) fn close_drawers(stack: &mut Stack) -> bool {
    // The one thing that must never be closed this way is the terminal powering-off frame, and the
    // guard for it belongs in the caller, not here: refusing to pop would only leave the sheet
    // *under* whatever is landing, and `power_off_requested` reads the top of the stack — so the
    // shutdown would be cancelled either way. `land` therefore refuses the card outright, and this
    // states the invariant it upholds.
    debug_assert!(!powering_off(stack), "a card must not land on a device that is switching off");
    let had = stack.last().is_some_and(|top| top.is_overlay());
    while stack.last().is_some_and(|top| top.is_overlay()) {
        stack.pop();
    }
    had
}

/// Apply a [`Transition`] to the stack. The root is never popped, so `back`
/// always has a defined target and the stack can never empty.
pub fn apply(stack: &mut Stack, t: Transition) {
    match t {
        Transition::None => {}
        Transition::Push(s) => {
            // Pushing a **drawer over a base** is the other direction and is untouched — that is an
            // overlay landing on a non-overlay, and the drawer owner
            // ([`App::apply_chord`](crate::App::apply_chord)) already enforces one sheet at a time.
            if !s.is_overlay() {
                close_drawers(stack);
            }
            // An overflow no-ops in release (the top screen just doesn't open); in sim/tests a
            // navigation tree grown past MAX_DEPTH fails loudly instead of silently dropping it.
            let r = stack.push(s);
            debug_assert!(r.is_ok(), "screen stack overflow — raise MAX_DEPTH");
        }
        Transition::Pop => {
            if stack.len() > 1 {
                stack.pop();
            }
        }
        Transition::Replace(s) => {
            if let Some(top) = stack.last_mut() {
                *top = s;
            }
        }
        Transition::Root(s) => {
            stack.truncate(1); // keep the Home root
            let r = stack.push(s); // can't overflow: len is 1 and MAX_DEPTH > 1
            debug_assert!(r.is_ok(), "screen stack overflow — raise MAX_DEPTH");
        }
        Transition::Home => stack.truncate(1),
    }
}

/// Logic context handed to [`Screen::handle`]: the mutable app state a screen adjusts. The
/// render half is [`Render`].
pub struct Ctx<'a> {
    pub state: &'a mut AppState,
    pub activity: &'a mut Activity,
    /// The persisted device settings — the settings screens edit this in place; a change is
    /// detected by [`App::apply_gesture`](crate::App::apply_gesture) and flagged for the host
    /// to save. Every other screen leaves it untouched.
    pub settings: &'a mut Settings,
    pub routes: &'a [RouteSummary],
    /// The resident ride catalog (read-only here) — the Rides screen lists it and its hold-to-delete
    /// footer records a delete by index against it (epic #447, P7).
    pub rides: &'a [RideSummary],
    /// The resident trip catalog (epic #526, TR3) — the grouped-route folders. The Route menu's top
    /// level lists these above the unfiled routes and its long-press → confirm dialog cascade-deletes
    /// one; every other screen leaves it untouched.
    pub trips: &'a [crate::trip::TripSummary],
    /// The loaded map's routing-profile names (routing-v2 N5) — the Bike-type settings screen cycles
    /// [`Settings::bike_profile_idx`](crate::Settings) within [`NavProfiles::len`](crate::NavProfiles).
    /// Empty before a map load / on a router-less image (the setting then cycles nowhere, inert).
    pub nav_profiles: &'a crate::NavProfiles,
    /// The App-owned POI-list snapshot, **read-only** here. The POI list's `Gesture::Press` reads
    /// the highlighted [`Poi`](obc_reader::Poi) out of it to hand to the detail screen — the one
    /// place `handle` reaches the draw-taken snapshot. Every other screen leaves it untouched.
    pub poi_scratch: &'a PoiScratch,
    /// The active route's resident waypoint table, **read-only** here — the Up-ahead timeline walks
    /// it (with [`corridor`](Ctx::corridor)) to resolve its cursor and the pressed row. Empty
    /// without a route; every other screen leaves it untouched.
    pub waypoints: &'a [obc_route::WptEntry],
    /// The App-owned **route-corridor POI snapshot** (epic #946, U2), **read-only** here — the
    /// other half of the Up-ahead merge, so `handle` sees exactly the rows `draw` drew. Empty until
    /// a snapshot lands; every other screen leaves it untouched.
    pub corridor: &'a [obc_reader::CorridorPoi],
    /// The live BLE **sensor scan hits** (epic #707, SE7), read-only here — the scan-list screen's
    /// `Gesture::Press` reads the highlighted hit's address out of it to save + connect. Empty outside
    /// a scan; every other screen leaves it untouched.
    pub sensor_scan_hits: &'a [crate::sensors::SensorScanHit],
    /// Whether the panel has a controllable light
    /// ([`App::backlight_available`](crate::App::backlight_available)) — the quick drawer's root
    /// row is one control shorter without one. A constant of the platform, threaded rather than
    /// stored, because it is the *screen's* content that depends on it.
    pub backlight: bool,
    /// The **Navigator** domain (#1397 S2) — the planning screens name what they want to it
    /// (`admit_intent`) rather than latching a request of their own. A rider's plan, cancel or
    /// commit therefore exists in exactly one place from the instant they press.
    pub navigator: &'a mut crate::navigator::NavigatorMachine,
    /// The **Recorder** domain (#1398) — the ride screens name the rider's start, save or discard
    /// to it (`request`) as the gesture happens. Nothing in [`Activity`] holds a ride one-shot, so
    /// the close exists in exactly one place from the instant they press.
    pub recorder: &'a mut crate::recorder::RecorderMachine,
    /// The **DFU** domain — the Firmware and update-confirm screens post their phase here.
    pub dfu: &'a mut crate::dfu::DfuState,
    /// The **StorageInfo** domain — the System screen asks for a free-space refresh on entry.
    pub storage: &'a mut crate::device_core::storage_info::StorageInfo,
    /// The **Weather** domain — the menu row that opens the dashboard names its refresh here, and
    /// the rain-map screens clamp against the zoom floor it derived.
    pub weather: &'a mut crate::weather::WeatherDomain,
    pub now_ms: u32,
}

impl Ctx<'_> {
    /// The base facts a context row's availability and value read, from an input context — the
    /// same answer [`Render::context_facts`] gives the frame that drew the row (#1515).
    pub(crate) fn context_facts(&self) -> context_drawer::ContextFacts<'_> {
        context_drawer::ContextFacts {
            state: self.state,
            activity: self.activity,
            settings: self.settings,
            recording: self.recorder.recording(),
        }
    }

    /// What the Up-ahead timeline is scoped to right now — the twin of [`Render::up_ahead_scope`],
    /// so `handle` walks exactly the rows `draw` drew.
    pub(crate) fn up_ahead_scope(&self) -> crate::corridor::UpAheadScope {
        crate::corridor::UpAheadScope { filter: self.state.up_ahead_filter, source: self.settings.up_ahead_source }
    }
}

/// A [`Ctx`] over borrowed state/activity/settings with every catalog empty and the clock at zero —
/// the shape essentially every screen test wants. The handful that need one field populated say so
/// with struct-update syntax, so the other eleven stay out of the way:
/// `Ctx { routes, ..test_ctx(&mut st, &mut act, &mut s) }`.
///
/// The five domain seams — Navigator, Recorder, DFU, `StorageInfo` and weather — are **leaked**,
/// one fresh set per call: they need `&mut` for a lifetime
/// the helper cannot own, and a test that asserts on one passes its own instead
/// (`Ctx { navigator: &mut nav, ..test_ctx(…) }`). A few dozen bytes per screen test, in a build
/// that has `std`.
#[cfg(test)]
pub(crate) fn test_ctx<'a>(state: &'a mut AppState, activity: &'a mut Activity, settings: &'a mut Settings) -> Ctx<'a> {
    // Shared empty borrows: the screens under test read these but never fill them, so one immutable
    // `'static` each serves every caller (and spares each test a local it has to keep alive — a
    // temporary can't outlive this call).
    static EMPTY_SCRATCH: PoiScratch = PoiScratch::new();
    static EMPTY_PROFILES: crate::NavProfiles = crate::NavProfiles::EMPTY;
    Ctx {
        state,
        activity,
        settings,
        routes: &[],
        rides: &[],
        trips: &[],
        nav_profiles: &EMPTY_PROFILES,
        backlight: true,
        poi_scratch: &EMPTY_SCRATCH,
        waypoints: &[],
        corridor: &[],
        sensor_scan_hits: &[],
        navigator: Box::leak(Box::new(crate::navigator::NavigatorMachine::new())),
        recorder: Box::leak(Box::new(crate::recorder::RecorderMachine::new())),
        dfu: Box::leak(Box::new(crate::dfu::DfuState::new())),
        storage: Box::leak(Box::new(crate::device_core::storage_info::StorageInfo::new())),
        weather: Box::leak(Box::new(crate::weather::WeatherDomain::new())),
        now_ms: 0,
    }
}

/// The currently-tracked climb, surfaced to the riding views (C3). Bundles the active
/// [`ClimbSeg`] with its resident detail [`ClimbProfile`], both borrowed from the App-owned climb
/// state — present exactly when [`Activity::active_climb`](crate::activity::Activity) is `Some`, so
/// the two are always consistent and a screen never draws a stale buffer. The Climb screen (C4)
/// reads `seg` for the base/top/gain/grade tiles and `profile` for the striped elevation panel +
/// cursor; nothing draws it yet at C3.
#[derive(Clone, Copy)]
pub struct ActiveClimb<'a> {
    /// The active climb's segment: interval, base/top elevation, gain, average grade.
    pub seg: &'a ClimbSeg,
    /// The active climb's resident detail profile (elevation-per-column, derived grade), scoped to
    /// this climb's `[start_m, end_m]` — refilled only on climb entry, so free to read per frame.
    pub profile: &'a ClimbProfile,
}

/// Render context handed to [`Screen::draw`]: the read-only state plus the map
/// `Reader`, the host's borrowed `RenderScratch`, and the in-flight Select hold-progress
/// (0.0–1.0) the guarded-action confirm ring fills with.
pub struct Render<'a> {
    /// The frame's borrowed render scratch — the host owns it and lends it for this call (#1146).
    /// Only the map-drawing screens touch it; it carries nothing between frames, so a screen that
    /// wants a presentation switch to stick states it per frame in an
    /// [`obc_render::RenderConfig`].
    ///
    /// `None` when the host lent no scratch (#1146 P2) — legitimate for a chrome-only frame, which
    /// is exactly the set of frames that never reach the map scene's draw, the one place this is
    /// unwrapped.
    pub scratch: Option<&'a mut RenderScratch>,
    /// The frame's **rain overlay lease** (WX10) — the host-constructed adapter over the active
    /// weather bundle's *current* frame, or `None` when nothing may render (no store, no current
    /// frame, expired bundle) **or when the base screen did not declare
    /// [`Caps::rain_overlay`]**. Like the scratch it is per-frame: the map-drawing base screen
    /// `take`s it and threads it into [`RenderScratch::render_rain_timed`], where the
    /// precipitation raster draws below the road band; `None` renders a byte-identical rain-free
    /// map. The freshness decision lives with the adapter (`obc-weather`'s `current_frame`), never
    /// in a screen; *which screen may see rain at all* is the declared capability, resolved once in
    /// [`App::render_scene_map_rain_timed`](crate::App::render_scene_map_rain_timed) — so a map
    /// base that never asked for rain (the Map, the Detour pair) is handed `None` and cannot leak
    /// the rain map's raster onto its own frame.
    pub rain: Option<&'a mut dyn obc_render::RainOverlaySource>,
    pub state: &'a AppState,
    pub activity: &'a Activity,
    /// The persisted device settings (read-only here) — the riding views read
    /// [`units`](Settings::units) to caption + scale their readouts.
    pub settings: &'a Settings,
    pub routes: &'a [RouteSummary],
    /// Each route's device-local retention meta (epic #638 S3), pairwise with [`routes`](Render::routes)
    /// — the Route overview's expiry row reads the previewed route's to show its "Auto-delete"
    /// countdown. Empty (every route reads the [`Never`](crate::Retention::Never) default) on a host
    /// that doesn't feed retention; the overview then simply omits the row.
    pub route_metas: &'a [crate::retention::RouteRetentionMeta],
    /// The resident ride catalog (read-only) — the Rides screen draws its two-line rows + the
    /// hold-to-delete footer from it (epic #447, P7).
    pub rides: &'a [RideSummary],
    /// The resident trip catalog (epic #526, TR3) — the grouped-route folders. The Route menu draws
    /// its folder rows above the unfiled routes and, scoped to one trip, its member routes' stage
    /// list; every other screen leaves it untouched.
    pub trips: &'a [crate::trip::TripSummary],
    /// The loaded map's routing-profile names (routing-v2 N5) — the Bike-type settings screen draws
    /// the selected profile's name (a stale index renders profile 0's — the router's fallback) and the created-route
    /// overview labels itself with it. Resident in the App because these frames draw without a
    /// `Reader` on the board.
    pub nav_profiles: &'a crate::NavProfiles,
    /// The active route's geometry (the Map strokes it), or `None` when no route is loaded.
    /// Host-owned, streamed on demand.
    pub route: Option<&'a RouteReader<'a>>,
    /// The active route's elevation profile (the Elevation screen draws it), rebuilt on route load
    /// and cached — `None` when no route is loaded. Resident, so the screen never re-reads to draw.
    pub profile: Option<&'a Profile>,
    /// The **viewed ride's** recorded-track elevation profile (epic #678 T2 / #680) — the Ride
    /// detail's band source, host-filled into the app's single resident ride-profile buffer on
    /// detail entry (the keyed ride-track answer) and invalidated on
    /// exit. `None` while the fill is still streaming (the band shows its loading note) and on
    /// every other screen.
    pub ride_profile: Option<&'a Profile>,
    /// The climb the rider is currently on, or `None` between climbs (C3). Bundles the two things
    /// the Climb screen (C4) draws — the active [`ClimbSeg`] (base/top/gain/grade) and its resident
    /// detail [`ClimbProfile`] — behind one `Option` so a `Some` is exactly "a climb is being
    /// tracked, both are valid". `None` whenever [`Activity::active_climb`](crate::activity::Activity)
    /// is `None` (no route, off any climb), so a screen never reads a stale detail buffer.
    pub climb: Option<ActiveClimb<'a>>,
    /// The active route's resident named-waypoint table (App-owned), in route order — the riding
    /// views draw its diamonds / chip / ticks / stat fields (later in the epic) and index it with
    /// [`Activity::next_waypoint`](crate::activity::Activity). Empty when no route is loaded, so a
    /// screen iterates it unconditionally.
    pub waypoints: &'a Waypoints,
    /// The travelled-path breadcrumb (bounded RAM); the Map strokes it under the route. Empty when
    /// nothing has been recorded yet, so the Map can skip it with [`Breadcrumb::is_empty`].
    pub breadcrumb: &'a Breadcrumb,
    /// Whether a ride is open — [`RecorderMachine::recording`](crate::RecorderMachine::recording),
    /// threaded as the level it is. The riding views' chrome, the Firmware page's install row and
    /// the two delete rows all read it; none of them may keep a second copy.
    pub recording: bool,
    /// The previewed route's decimated shape polyline (#685 §4; #678 rework 3 widened it to
    /// stored routes) — ≤ 64 `(lon, lat)` µdeg points, host-decimated and keyed to the active
    /// route (the App hands an empty slice when it's missing or stale). Only the Route overview
    /// draws it — the computed page's mid-gap sketch, the full page's track-pager band.
    pub nav_preview: &'a [(i32, i32)],
    /// The viewed ride's decimated recorded-track shape polyline (#678 rework 3) — ≤ 64
    /// `(lon, lat)` µdeg points, host-filled alongside the ride profile on detail entry
    /// (the keyed ride-track answer) and keyed to [`Activity::viewed_ride`](crate::Activity) (the App
    /// hands an empty slice when it's missing or stale). Only the Ride detail's track pager page
    /// draws it.
    pub ride_preview: &'a [(i32, i32)],
    /// The planned-but-uncommitted detour's decimated polyline (#882) — host-filled when the
    /// detour plan completes, keyed to the active route (empty when missing or stale). Only the
    /// Detour preview screen draws it, over the still-active original route.
    pub detour_preview: &'a [(i32, i32)],
    /// The single [`App`](crate::App)-owned POI-list snapshot buffer, **read-only** here (#803).
    /// Only the [`PoiList`](crate::screen::poi_list) screen reads it, drawing the frozen snapshot its
    /// [`prepare`](Screen::prepare) pass already took (see [`PoiScratch`] / [`Prepare`]); every other
    /// screen leaves it untouched. Draw is side-effect-free — the acquisition moved out of the draw
    /// path to the pre-draw prepare phase, so `Render` no longer carries mutable POI scratch.
    pub poi_scratch: &'a PoiScratch,
    /// The frozen **route-corridor POI snapshot** (epic #946, U2) the Up-ahead timeline merges with
    /// [`waypoints`](Render::waypoints) — ascending by along-route distance, empty until one lands.
    pub corridor: &'a [obc_reader::CorridorPoi],
    /// Whether that snapshot has **settled**: taken, or settled empty on a query error (U2). `false`
    /// only while the query still waits for its inputs (no `Reader` / no route geometry this frame),
    /// which is what keeps the Up-ahead empty state from flashing an answer the next frame
    /// contradicts.
    pub corridor_settled: bool,
    /// The App-owned per-category **"next ahead" cache** (epic #946, U5) — the distilled map-POI
    /// half of the six `Next: <category>` stat tiles, refreshed on the progress-keyed policy in
    /// [`NextAhead`](crate::next_ahead::NextAhead) rather than per frame. Read-only; only
    /// [`Readout`](crate::stat_fields::Readout) consumers touch it.
    pub next_ahead: &'a crate::next_ahead::NextAhead,
    /// The per-slot BLE **sensor status** (epic #707, SE7) — the Sensors settings screen draws the
    /// HR / power / cadence rows' status lines from it. Fed each pass by the host; empty defaults
    /// elsewhere, so the screen indexes it by slot unconditionally.
    pub sensor_status: &'a [crate::sensors::SensorStatus],
    /// The live BLE **sensor scan hits** (epic #707, SE7) — the scan-list screen's rows (name/address
    /// + RSSI, filtered to the row's quantity). Empty outside a scan.
    pub sensor_scan_hits: &'a [crate::sensors::SensorScanHit],
    /// Panel size in device pixels. Integer, because every screen lays out in whole pixels;
    /// the Map computes its `f32` viewport locally.
    pub w: i32,
    pub h: i32,
    pub now_ms: u32,
    /// The current **UTC** unix seconds ([`App::wall_unix_now`](crate::App::wall_unix_now)) — the
    /// instant the Route overview's expiry row subtracts from a route's `expires_at` to show the
    /// time left. Display-only here: unlike the auto-expiry sweep it is *not* gated on a trusted
    /// clock (a stale boot set-point just yields a stale countdown, never a deletion).
    pub now_utc: u32,
    /// The live wall-clock time this frame (set-point advanced by elapsed millis — see
    /// [`WallClock`](crate::WallClock)). The Home screensaver draws it as `HH:MM`; for boot-relative
    /// millis a screen uses [`now_ms`](Render::now_ms) instead.
    pub now: DateTime,
    /// Whether [`now`](Render::now) has an **established** origin — a persisted/manual/GPS time has
    /// been applied, versus a fresh clock that has never known the time (see
    /// [`App::clock_is_set`](crate::App::clock_is_set)). The Home date line draws only when set, so a
    /// date with no trusted origin is never shown; the `HH:MM` clock still draws either way.
    pub clock_set: bool,
    pub hold_progress: f32,
    /// No current GPS fix this frame: no fix yet (acquiring) or the last has gone stale (lost). The
    /// riding views draw the "No GPS Fix" banner when set, and the Map suppresses the off-route pill
    /// (the match is stale). Computed by [`App::has_live_fix`](crate::App::has_live_fix).
    pub no_fix: bool,
    /// Microsecond clock for the map render's per-stage timing, passed to
    /// [`RenderScratch::render_timed`]. Hosts that don't profile pass
    /// [`NoopClock`](obc_render::NoopClock); the device passes its `Instant`-based clock. Part of the
    /// strippable render-instrumentation seam.
    pub clock: &'a dyn Clock,
    /// What the base screen's map render drew this frame, for the host's stats panel / frame log.
    /// Reset to default by the host each frame; map-base screens write it and every other screen
    /// leaves it untouched.
    pub stats: RenderStats,
    /// The running firmware version string (T8 item 6) — the System settings screen's `Firmware`
    /// ledger row. Empty until the host feeds it via [`App::set_fw_version`](crate::App::set_fw_version).
    pub fw_version: &'a str,
    /// The loaded map's display name (T8 item 6) — the left half of the System screen's `Map` row.
    /// Empty until [`App::set_map_info`](crate::App::set_map_info) runs on map load.
    pub map_name: &'a str,
    /// The loaded map's OBCM format version — the right half of the `Map` row (`0` = no map yet).
    pub map_obcm_version: u8,
    /// Free space on the medium in bytes (T8 item 6), or `None` until a measurement answers the
    /// System screen's on-entry refresh — the figure
    /// [`StorageInfo`](crate::device_core::storage_info::StorageInfo) owns.
    pub card_free_bytes: Option<u64>,
    /// The host-fed resident **weather snapshot** (WX11, epic #1185) — the 24 hourly records +
    /// sampled rain-frame table the weather screens derive every claim from
    /// ([`rain_outlook`](crate::weather::rain_outlook) against this frame's [`now_utc`](Render::now_utc)),
    /// or `None` when no store is mounted / nothing was ever fetched (the explicit no-data state).
    /// Host-owned like the rain lease: the sim samples its loaded bundle, the board's WX8 mount
    /// will feed the same shape.
    pub weather: Option<&'a crate::weather::WeatherSnapshot>,
    /// A weather refresh is in flight (WX8's request/upload cycle; the sim's injection flag).
    /// The dashboard shows its one non-blocking cue off this — cached content stays visible
    /// (locked UX), so this is a title-slot caption, never a blocking spinner.
    pub weather_refreshing: bool,
    /// The rider's travel direction (degrees CW from north) for the route-relative wind arrows
    /// (WX12, epic #1185): active-route tangent at the matched progress, else the moving GPS
    /// course, else `None` — the hourly rows then draw neutral arrows, never a fabricated
    /// head/tail ([`wind_class`](crate::weather::wind_class)'s locked fallback).
    pub travel_deg: Option<f32>,
    /// Whether the panel has a controllable light — see [`Ctx::backlight`]. The quick drawer draws
    /// three icons instead of four without one.
    pub backlight: bool,
}

impl Render<'_> {
    /// The base facts a context row's availability and value read, from a draw context (#1515).
    pub(crate) fn context_facts(&self) -> context_drawer::ContextFacts<'_> {
        context_drawer::ContextFacts {
            state: self.state,
            activity: self.activity,
            settings: self.settings,
            recording: self.recording,
        }
    }

    /// What the Up-ahead timeline is scoped to this frame — the rider's live filter and their
    /// persisted source preference, read as one value (epic #946 U4 / #1515 D4a).
    pub(crate) fn up_ahead_scope(&self) -> crate::corridor::UpAheadScope {
        crate::corridor::UpAheadScope { filter: self.state.up_ahead_filter, source: self.settings.up_ahead_source }
    }

    /// The narrow live-data view the stat-field catalogue formats from — the one constructor of
    /// [`Readout`](crate::stat_fields::Readout), so `stat_fields` stays decoupled from the full
    /// draw context (and its `RenderScratch`).
    pub fn readout(&self) -> crate::stat_fields::Readout<'_> {
        crate::stat_fields::Readout {
            fix: self.state.user_fix,
            activity: self.activity,
            units: self.settings.units,
            route: self.route,
            profile: self.profile,
            climb: self.climb,
            waypoints: self.waypoints,
            next_waypoint: self.activity.next_waypoint,
            now: self.now,
            now_ms: self.now_ms,
            bike_profile_idx: self.settings.bike_profile_idx,
            language: self.settings.language,
            next_ahead: self.next_ahead,
        }
    }
}

/// One frame's draw context plus the base-map scene it streams.
///
/// Keeping the scene in this thin wrapper is what lets only the map-bearing screens be generic
/// over [`MapScene`]. Every chrome screen still receives `&mut Render` through `Deref`, so the
/// generic source does not infect the whole screen catalogue. Since FS7.5 (#1420) a map is one
/// file, so the scene a host supplies here is a plain [`Reader`] — the generic stays because it is
/// what keeps the chrome screens out of it, not because two scene types are still in play.
pub struct RenderFrame<'a, S: MapScene> {
    pub scene: Option<&'a S>,
    pub render: Render<'a>,
}

impl<'a, S: MapScene> Deref for RenderFrame<'a, S> {
    type Target = Render<'a>;

    fn deref(&self) -> &Self::Target {
        &self.render
    }
}

impl<S: MapScene> DerefMut for RenderFrame<'_, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.render
    }
}

/// The narrow **pre-draw acquisition** context (#803): the streamed-map [`Reader`] (when the host
/// built it this frame), the shared POI snapshot buffer, and the live fix — everything a screen
/// needs to resolve its reader-backed one-shot state **before** drawing. Handed to
/// [`Screen::prepare`], which runs on the base screen once per frame ahead of the draw loop, so the
/// side-effectful POI snapshot / hours read happens here and [`draw`](Screen::draw) then consumes
/// immutable prepared state (the narrowed, mutable-scratch-free [`Render`]). The POI screens use
/// the map `Reader`; the Skip-ahead chooser uses the streamed route and live route progress.
pub struct Prepare<'a, 'd> {
    /// The streamed-map `Reader`, or `None` when the host didn't build it this frame — the POI
    /// acquisitions retry next frame until [`base_needs_reader`](crate::App::base_needs_reader)
    /// (which reads the same [`ReaderNeed`] declaration) stops asking the board to build it.
    pub reader: Option<&'a Reader<'d>>,
    /// The active streamed route geometry, if the host opened it this frame. The Skip-ahead chooser
    /// resolves its exact rejoin coordinate and selected-stretch bounds from this; POI screens
    /// ignore it.
    pub route: Option<&'a RouteReader<'a>>,
    /// The single [`App`](crate::App)-owned POI-list snapshot buffer — the POI list fills it here.
    pub poi_scratch: &'a mut PoiScratch,
    /// The rider's current fix, `(lon, lat)` µdeg — the POI list's nearest-16 query origin.
    pub user_fix: Option<Fix>,
    /// Copy of the active route slot and live matched progress at this frame's prepare boundary.
    /// The Detour chooser advances its selection anchor with the rider before resolving geometry.
    pub active_route: Option<usize>,
    pub progress_m: u32,
    pub route_total_m: u32,
    /// The planned detour's decimated polyline (#882) — the Detour preview folds it into its
    /// fitted camera bounds; empty for every other screen (and while nothing is planned).
    pub detour_preview: &'a [(i32, i32)],
}

/// A screen's classification, declared **in its `screens!` table row** so it can never drift from
/// the enum. The two kinds behavior hangs off: [`Overlay`](ScreenKind::Overlay) screens composite
/// over the screen below instead of replacing the view, and [`Settings`](ScreenKind::Settings)
/// screens gate the debounced settings save
/// (the pass). `Riding` (the live sensor
/// views) and `Nav` (Home + the menus/prompts) carry no behavior yet — they exist so every row
/// states what its screen *is*.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenKind {
    /// A live riding view (Map, Skip ahead, Statistics) — full-screen, fed by the fix.
    Riding,
    /// Navigation chrome: the Home root, the menus, and the full-screen prompts.
    Nav,
    /// Drawn *over* the screen below (the stack composites it on top).
    Overlay,
    /// Part of the settings subtree — edits are held un-persisted while one is on top.
    Settings,
}

impl ScreenKind {
    /// Whether this kind composites over the screen below rather than replacing the view.
    pub fn is_overlay(self) -> bool {
        matches!(self, ScreenKind::Overlay)
    }

    /// Whether this kind belongs to the settings subtree (a pending save is held while on it).
    pub fn is_settings(self) -> bool {
        matches!(self, ScreenKind::Settings)
    }
}

/// What a screen's **base content** is — the thing its lowest-opaque draw *is*. The map-plane host
/// gates whole pipelines off this one declared fact rather than scattered `matches!` on the enum:
/// building the streamed-map [`Reader`], counting a screen as live-data (a fresh fix must redraw
/// it), and showing the BLE connected indicator all read the base screen's [`BaseContent`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseContent {
    /// Draws the streamed base map and therefore reads the [`Reader`] for the map itself.
    Map,
    /// A live riding view fed by the fix but not the map (Statistics, Climb): a fresh fix redraws
    /// it, but it draws no map I/O and shows no BLE indicator.
    LiveRiding,
    /// Static chrome — Home, the menus, the lists, the prompts, the settings subtree. No live
    /// map/fix redraw; carries the BLE connected indicator in its title bar.
    Chrome,
}

/// Whether — and until when — a screen needs the streamed-map [`Reader`] built and passed to
/// [`render_map_timed`](crate::App::render_map_timed). Declared per screen so the render-on-demand
/// board host skips the per-frame `Reader` build (an SD style-table parse + its stack spike) on
/// every frame that doesn't need it. The two POI variants take a **one-shot** read at
/// [`prepare`](Screen::prepare) time (the snapshot / the hours), so their need is conditional on
/// that read still being pending — [`base_needs_reader`](crate::App::base_needs_reader) keeps the
/// runtime pending check, but which check to run is chosen from this declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReaderNeed {
    /// Never needs the `Reader` (all chrome and live-riding non-map screens).
    Never,
    /// Always needs it — any [`Map`](BaseContent::Map) base screen.
    Always,
    /// Needs it until the POI list's category snapshot has been taken (issue #425).
    PoiSnapshot,
    /// Needs it until the POI detail's opening-hours read has resolved (issue #444).
    PoiHours,
}

/// The **render key** a screen's content is made of — the exact facts its draw reads, declared in
/// its `screens!` table row so the repaint decision is a property of the screen rather than of the
/// call sites that happen to mutate those facts.
///
/// The pass builds the visible stack's key before and after its stages and dirties the map when the
/// two differ (see [`render_key`](crate::render_key)). A screen whose content moves only on input,
/// on its own [`tick_timers`](Screen::tick_timers), or on a mutation that already carries an
/// explicit dirty request declares [`Static`](RenderKeyKind::Static) and contributes only its
/// identity — which is enough for a navigation to repaint, because the identity is part of the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderKeyKind {
    /// No fact of its own: the screen's identity in the visible stack is its whole key.
    Static,
    /// Battery, the connected indicator, and the screensaver backdrop's per-open jitter.
    Home,
    /// The camera, the fix that drives it, the pan HUD, the route-relative map chrome, and the
    /// top-left low-battery cue.
    Map,
    /// The riding grid: progress, off-route, no-fix, the active climb, the next waypoint, and the
    /// live sensor values of the fields actually pinned to the grid.
    Statistics,
    /// The active climb's identity, the cursor's position along it, and the fix behind both.
    Climb,
    /// The saved sensors' per-slot status and the live scan list's revision.
    SensorSettings,
    /// The installed weather data's identity.
    Weather,
    /// The Up-ahead timeline: live route progress, the route's length, and the corridor snapshot
    /// the rows are merged from.
    UpAhead,
    /// A **drawer**: the page it shows, the row selected on it, and the value it has staged
    /// against the one already committed.
    ///
    /// This kind does one thing no other does — it **shadows** every other kind. A drawer freezes
    /// the base it covers: while one is visible the key names only these facts, so a camera move,
    /// a fresh fix or an arriving weather bundle under an open drawer cannot dirty the map, and
    /// closing it changes the key's shape exactly once. That is the whole of "the frozen base" —
    /// there is no capture buffer and no second framebuffer, only a key that stops looking.
    Drawer,
}

/// Which durable catalog a screen's held **indices** are remapped against after a store rescan
/// (#450). The rescan renumbers the route/ride catalogs; a screen that caches an index into one
/// must be re-pointed (or dropped) through the App's remap closure. Declared per screen so the
/// remap fan-out ([`App::remap_route_indices`](crate::App)) can never silently forget a screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemapKind {
    /// Holds no catalog index — nothing to remap.
    None,
    /// Holds a **route** catalog index (the Skip chooser / Route menu / overview / swap / upload cards).
    Route,
    /// Holds a **ride** catalog index (the Rides list / ride detail).
    Ride,
}

/// The compact **capability metadata** for one screen, declared **in its `screens!` table row** so
/// cross-cutting UI policy is a single declaration that can never drift from the enum. The map-plane
/// host, the idle-return policy, the timer sweep, the hold-fill gate, the reader-build seam, and the
/// rescan remap all read a screen's [`Caps`] instead of open-coding a `matches!` on the variant.
///
/// Built with the const archetype constructors ([`nav`](Caps::nav), [`map`](Caps::map),
/// [`riding`](Caps::riding), [`settings`](Caps::settings), [`modal`](Caps::modal)) and refined with
/// the const chaining setters — a row reads `Caps::map().timed()` or
/// `Caps::settings().hold_fill()`. The struct is **not** stored in the [`Screen`] enum (it would
/// inflate every stack slot); it compiles to a `const` per variant reachable by
/// [`caps`](Screen::caps) / the [`CAPS`](Screen::CAPS) table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    /// The [`ScreenKind`] the overlay/settings behaviors hang off.
    pub kind: ScreenKind,
    /// What the screen's base draw is — gates map I/O, live-data redraw, and the BLE indicator.
    pub base: BaseContent,
    /// **Idle-return exempt**: a modal card/wait the idle-return timeout must never yank away (the
    /// passkey card, the upload popups, the routing spinner, the whole DFU flow).
    pub idle_exempt: bool,
    /// A **deliberate ride view** (Map, Statistics, Climb, Ride control): stays put on the idle
    /// timeout *while a ride is tracked* instead of returning to the Map.
    pub ride_view: bool,
    /// A **deliberate browse view when not tracking** (the route-less browse Map): the idle timeout
    /// treats it as intentional, not idleness, so it isn't returned to Home.
    pub browse_exempt: bool,
    /// Whether — and until when — the screen needs the streamed-map [`Reader`] at draw.
    pub reader: ReaderNeed,
    /// Declares the screen has **timed content**: its [`tick_timers`](Screen::tick_timers) arm can
    /// fire a time-driven repaint (and it must therefore have a non-idle arm — the invariant tests
    /// pin the two together).
    pub timed: bool,
    /// Declares the screen can draw a live **hold fill** for a guarded selection — it has a
    /// [`wants_hold_fill`](Screen::wants_hold_fill) arm.
    pub hold_fill: bool,
    /// Declares the screen a **genuinely blocking** modal: while it is on top the device-wide
    /// drawer chords ([`Chord`](crate::input::Chord)) are refused, so a squeeze cannot open a
    /// drawer over a pairing passkey, a running map transfer, or the terminal install card. Every
    /// *other* screen is eligible — the quick drawer is global by design.
    pub blocks_chords: bool,
    /// Declares the screen a **decision the rider must answer**: while it is on top the global
    /// Back-hold escape ([`App::apply_gesture`](crate::App::apply_gesture)) does not leave it. A
    /// superset of [`blocks_chords`](Caps::blocks_chords) — anything that refuses a drawer chord
    /// refuses the escape too, so [`blocking`](Caps::blocking) sets both — plus the recovered-ride
    /// card, over which a sheet is harmless but an exit would strand the recovered object.
    pub blocks_escape: bool,
    /// Declares the screen **wants the rain overlay** (WX10/WX11): the frame's rain lease is
    /// handed to it and the precipitation raster draws inside its map scene. Off for every other
    /// screen — including the ordinary Map and the Detour pair, which draw the same scene through
    /// the same helper — so the overlay is a property of the screen the rider is on, not of the
    /// frame the host happened to lease weather for. That makes leaving the rain map clean by
    /// construction: there is no exit hook to forget, and a future map screen is rain-free until
    /// its row says otherwise.
    pub rain_overlay: bool,
    /// Which catalog the screen's held indices remap against after a rescan (#450).
    pub remap: RemapKind,
    /// The exact facts this screen's draw reads — the pass compares them before and after its
    /// stages and dirties the map when they move (see [`RenderKeyKind`]).
    pub render_key: RenderKeyKind,
}

impl Caps {
    /// Navigation chrome — Home, the menus, the lists, the info prompts. The neutral base every
    /// other archetype refines from.
    pub const fn nav() -> Self {
        Caps {
            kind: ScreenKind::Nav,
            base: BaseContent::Chrome,
            idle_exempt: false,
            ride_view: false,
            browse_exempt: false,
            reader: ReaderNeed::Never,
            timed: false,
            hold_fill: false,
            blocks_chords: false,
            blocks_escape: false,
            rain_overlay: false,
            remap: RemapKind::None,
            render_key: RenderKeyKind::Static,
        }
    }

    /// A **drawer**: an overlay sheet composited over the still-visible base, which the frame
    /// draws through the dim LUT for as long as one is up (see
    /// [`dim_color`](crate::screen::dim_color)). Its key kind shadows the base's, which is what
    /// freezes it.
    pub const fn overlay() -> Self {
        Caps { kind: ScreenKind::Overlay, timed: true, render_key: RenderKeyKind::Drawer, ..Caps::nav() }
    }

    /// A map-base screen: reads the `Reader` every frame, and is both a tracking ride view and a
    /// deliberate browse view when not tracking.
    pub const fn map() -> Self {
        Caps {
            kind: ScreenKind::Riding,
            base: BaseContent::Map,
            ride_view: true,
            browse_exempt: true,
            reader: ReaderNeed::Always,
            render_key: RenderKeyKind::Map,
            ..Caps::nav()
        }
    }

    /// A live **riding** view fed by the fix but not the map (Statistics, Climb): a tracking ride
    /// view, redrawn on a fresh fix, no map I/O.
    pub const fn riding() -> Self {
        Caps {
            kind: ScreenKind::Riding,
            base: BaseContent::LiveRiding,
            ride_view: true,
            render_key: RenderKeyKind::Statistics,
            ..Caps::nav()
        }
    }

    /// A **settings** subtree screen — a pending save is held un-persisted while one is on top.
    pub const fn settings() -> Self {
        Caps { kind: ScreenKind::Settings, ..Caps::nav() }
    }

    /// A **modal** card or wait — idle-return exempt (must stay until dismissed/answered): the
    /// passkey card, the upload popups, the routing spinner, and the DFU flow.
    pub const fn modal() -> Self {
        Caps { idle_exempt: true, ..Caps::nav() }
    }

    /// Mark the screen a deliberate ride view (stays put on the idle timeout while tracking) — for
    /// the Ride-control page, whose base is chrome but which is a live ride view.
    pub const fn ride_view(mut self) -> Self {
        self.ride_view = true;
        self
    }

    /// Mark the screen idle-return exempt — for the manual Route-swap prompt, which shares the
    /// upload popups' "never yank mid-decision" rule.
    pub const fn exempt(mut self) -> Self {
        self.idle_exempt = true;
        self
    }

    /// Declare the screen has timed content (a [`tick_timers`](Screen::tick_timers) arm).
    pub const fn timed(mut self) -> Self {
        self.timed = true;
        self
    }

    /// Declare the screen can draw a hold fill (a [`wants_hold_fill`](Screen::wants_hold_fill) arm).
    pub const fn hold_fill(mut self) -> Self {
        self.hold_fill = true;
        self
    }

    /// Declare the screen a genuinely blocking modal — see [`blocks_chords`](Caps::blocks_chords).
    /// A screen that refuses the drawer chords refuses the global escape as well, so this sets
    /// both.
    pub const fn blocking(mut self) -> Self {
        self.blocks_chords = true;
        self.blocks_escape = true;
        self
    }

    /// Declare the screen a decision the global Back-hold escape must not leave — see
    /// [`blocks_escape`](Caps::blocks_escape). For the card that is not otherwise blocking.
    pub const fn blocks_escape(mut self) -> Self {
        self.blocks_escape = true;
        self
    }

    /// Declare the screen wants the frame's **rain overlay** lease (see
    /// [`rain_overlay`](Caps::rain_overlay)) — the rain map's row, and nothing else's.
    pub const fn rain_overlay(mut self) -> Self {
        self.rain_overlay = true;
        self
    }

    /// Set the screen's [`ReaderNeed`].
    pub const fn reader(mut self, need: ReaderNeed) -> Self {
        self.reader = need;
        self
    }

    /// Set the screen's rescan [`RemapKind`].
    pub const fn remap(mut self, remap: RemapKind) -> Self {
        self.remap = remap;
        self
    }

    /// Set the screen's [`RenderKeyKind`] — for the rows whose content is not the one their
    /// archetype names (the Climb view among the riding screens, the weather pages among the
    /// chrome, the Sensors pages among the settings).
    pub const fn key(mut self, render_key: RenderKeyKind) -> Self {
        self.render_key = render_key;
        self
    }
}

/// The one screen table. Each row is `Variant(StateType) => Caps`; the macro expands it into the
/// [`Screen`] enum, the `handle`/`draw`/`prepare` delegation matches, and the per-screen
/// [`Caps`](Screen::caps) (from which [`kind`](Screen::kind) and every cross-cutting policy
/// derives). **Adding a normal screen = adding one row here** (plus its own module, and a
/// [`tick_timers`](Screen::tick_timers) arm only if the row declares `.timed()`) — there is no
/// second list to keep in sync, and a cross-cutting policy addition is an explicit capability on the
/// row, not a forgotten `matches!` elsewhere. Deliberately a dumb token-pasting table, not a
/// framework.
macro_rules! screens {
    ($( $(#[$doc:meta])* $variant:ident($state:ty) => $caps:expr, )+) => {
        /// The on-device screens. Each variant owns its typed state and forwards to that screen's
        /// inherent `handle`/`draw`. Generated by `screens!` — the variants, delegation, and
        /// per-screen [`Caps`] all come from the one table.
        pub enum Screen {
            $( $(#[$doc])* $variant($state), )+
        }

        /// A screen's **identity**, without its state — one byte per visible row in a
        /// [`RenderKey`](crate::render_key). Generated from the same table as [`Screen`], so a row
        /// added there is comparable here with nothing to keep in sync.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(u8)]
        pub enum ScreenRow {
            $( $variant, )+
        }

        impl Screen {
            /// Handle one gesture, returning the navigation [`Transition`] it triggers.
            ///
            /// [`Gesture::BackHold`] never arrives here: it is the **global escape** to the main
            /// menu, resolved in [`App::apply_gesture`](crate::App::apply_gesture) above this
            /// dispatch (#1515 D3). Every screen's `BackHold` arm is therefore inert by
            /// construction — Rust wants the match exhaustive, not the arm reachable — and this
            /// assert is what says so loudly if a host ever routes one past the escape.
            pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
                debug_assert!(!matches!(g, Gesture::BackHold), "Back-hold is the global escape, not a screen gesture");
                match self {
                    $( Screen::$variant(s) => s.handle(g, cx), )+
                }
            }

            /// Draw the screen into the frame's [`Canvas`]. The two host generics stop here: every
            /// screen below draws through `&mut impl Surface`, except the Map, which reaches the raw
            /// target via [`Canvas::split`] for its `RenderScratch` calls (and writes [`Render::stats`]).
            pub fn draw<D, F, S>(&self, cv: &mut Canvas<D, F>, rx: &mut RenderFrame<'_, S>)
            where
                D: DrawTarget,
                F: Fn(u16) -> D::Color,
                S: MapScene,
            {
                match self {
                    $( Screen::$variant(s) => s.draw(cv, rx), )+
                }
            }

            /// This screen's [`Caps`], exactly as declared in its `screens!` table row — the single
            /// authority for its cross-cutting UI policy (base content, idle-return, reader need,
            /// timed/hold-fill, rescan remap). Every other classifier
            /// ([`kind`](Screen::kind), [`is_overlay`](Screen::is_overlay), the host's
            /// live-data/reader/idle gates) reads it instead of re-`matches!`ing the variant.
            pub fn caps(&self) -> Caps {
                match self {
                    $( Screen::$variant(_) => $caps, )+
                }
            }

            /// This screen's [`ScreenKind`], from its declared [`Caps`].
            pub fn kind(&self) -> ScreenKind {
                self.caps().kind
            }

            /// This screen's identity as a [`ScreenRow`] — what a render key records for each
            /// visible row, so a navigation moves the key without any screen saying so.
            pub fn row(&self) -> ScreenRow {
                match self {
                    $( Screen::$variant(_) => ScreenRow::$variant, )+
                }
            }

            /// This screen's variant name (e.g. `"Map"`, `"PoiList"`, `"NavPlanning"`), generated
            /// from the one table so it can't drift. The web demo host (`obc-web-demo`) publishes it
            /// (`obc_demo_state`) so the landing page can advance a guided demo only once the app
            /// actually reached the target screen.
            pub fn name(&self) -> &'static str {
                match self {
                    $( Screen::$variant(_) => stringify!($variant), )+
                }
            }

            /// Every screen's variant name, in `screens!` table order — the same strings
            /// [`name`](Screen::name) returns. The web demo host exports this
            /// (`obc_demo_screens`) as the landing page's drift-guard: a tour scripted against a
            /// screen name that no longer exists fails CI instead of silently stalling.
            pub const NAMES: &'static [&'static str] = &[ $( stringify!($variant), )+ ];

            /// Every screen's declared [`Caps`], in `screens!` table order — paired index-for-index
            /// with [`NAMES`](Screen::NAMES). The capability invariant tests enumerate this without
            /// having to construct each variant's state.
            pub const CAPS: &'static [Caps] = &[ $( $caps, )+ ];
        }
    };
}

screens! {
    Home(HomeScreen) => Caps::nav().timed().key(RenderKeyKind::Home),
    Map(MapScreen) => Caps::map().timed(),
    Statistics(StatisticsScreen) => Caps::riding().timed(),
    /// The Climb view (epic #506, C4): the current climb's grade-striped elevation profile + cursor
    /// + four climb-scoped tiles. A full-screen riding view like the Map/Statistics siblings; C5
    /// wires it into the Back-cycle and the auto-switch, so nothing reaches it yet except the
    /// debug-open bench path.
    Climb(ClimbScreen) => Caps::riding().key(RenderKeyKind::Climb),
    /// The pause page: ride-so-far ledger + the guarded Resume / Finish / Discard rows.
    RideControl(RideControl) => Caps::nav().ride_view().hold_fill(),
    /// The route-less start card (Menu → Map → press): "Start ride" / "Back". *Start ride* begins a
    /// tracking session with no route via [`start_ride_routeless`].
    RideStart(RideStartScreen) => Caps::nav(),
    /// The one-shot boot decision for a durable recording recovered after reset. Back cannot
    /// dismiss it; Continue preserves restored totals, while Discard is hold-guarded.
    RideRecovery(RideRecoveryScreen) => Caps::modal().hold_fill().blocks_escape(),
    Menu(MenuScreen) => Caps::nav().timed(),
    /// The "Up ahead" timeline (epic #946, U3): the route-ordered merge of the resident waypoint
    /// table and the App-owned corridor-POI snapshot, with the Hold category picker as an in-screen
    /// mode. Reads the snapshot the App arms from its `corridor_key`; holds no rows itself.
    UpAhead(UpAheadScreen) => Caps::nav().key(RenderKeyKind::UpAhead),
    /// Detour chooser (#882): a map base with streamed skipped-stretch ink and an auto-fit camera.
    Detour(DetourScreen) => Caps::map().remap(RemapKind::Route),
    /// Detour preview (#882): the planned detour + cost line over the map; Press commits the splice.
    DetourPreview(DetourPreviewScreen) => Caps::map().remap(RemapKind::Route),
    /// The POIs browser's category list (Menu → POIs).
    PoiMenu(PoiMenuScreen) => Caps::nav(),
    /// One category's distance-sorted nearest-16 with live bearing arrows.
    PoiList(PoiListScreen) => Caps::nav().reader(ReaderNeed::PoiSnapshot),
    /// A single POI's detail: full name, subtype, live bearing arrow, today's hours + open/closed.
    PoiDetail(PoiDetailScreen) => Caps::nav().reader(ReaderNeed::PoiHours),
    /// The POI "Create a route?" confirm (epic #116, R4): *Create route* records the one-shot
    /// [`NavRequest`](crate::activity::NavRequest) and swaps to the planning screen.
    NavConfirm(NavConfirmScreen) => Caps::nav(),
    /// The route-**planning** screen (#499): the spinning-needle wait while the host steps the
    /// resumable router; Back cancels (pops to the detail + rings the pass). The
    /// host's answer (the pass's fact stage) replaces it with the computed-route overview
    /// or the failure card.
    NavPlanning(NavPlanningScreen) => Caps::modal().timed(),
    /// The route-planning failure card (epic #116, R4): the locked two-tier copy ("Too far to
    /// route here." / "Couldn't find a route."), info-only — any press/Back returns to the detail.
    NavFail(NavFailScreen) => Caps::nav(),
    RouteMenu(RouteMenuScreen) => Caps::nav().remap(RemapKind::Route),
    /// The trip cascade-delete confirm dialog (epic #526, TR3): reached by long-pressing a trip
    /// folder row in the Route menu's top level. A warning-red hold-guarded Delete row + a Cancel
    /// row; a completed hold records the trip's durable id for the host to cascade-delete (trip +
    /// member routes).
    TripDelete(TripDeleteScreen) => Caps::nav().hold_fill(),
    /// The Rides screen (Menu → Rides): the stored-rides list — name + sync glyph over an olive
    /// `D MON · distance` line; press opens the Ride detail. Epic #447 P7 (#454), rows
    /// redesigned by #680.
    Rides(RidesScreen) => Caps::nav().remap(RemapKind::Ride),
    /// The Ride detail (Rides → press, #680): the recorded sibling of the Route overview —
    /// elevation band of the tracked ride, stat ledger, and the guarded Delete-ride row.
    RideDetail(RideDetailScreen) => Caps::nav().timed().hold_fill().remap(RemapKind::Ride),
    RouteOverview(RouteOverviewScreen) => Caps::nav().timed().hold_fill().remap(RemapKind::Route),
    RouteSwap(RouteSwapScreen) => Caps::nav().exempt().timed().hold_fill().remap(RemapKind::Route),
    /// The idle route-upload prompt (epic #447, P4): "ROUTE RECEIVED" — Start navigation / Dismiss.
    /// **Host-pushed** by the pass's fact stage; auto-closes (= dismisses) after
    /// [`UPLOAD_POPUP_TIMEOUT_MS`]. Advisory — the route is already committed and in the Route menu.
    RouteReceived(RouteReceivedScreen) => Caps::modal().timed().remap(RemapKind::Route),
    /// The active-route-replaced info card (epic #447, P4). Adoption already happened when it
    /// opens (the app dropped the stale matcher/profile; the host reopened the geometry) — this
    /// only *tells* the rider. Dismiss on any press/Back, or the same auto-close.
    RouteUpdated(RouteUpdatedScreen) => Caps::modal().timed().remap(RemapKind::Route),
    /// The trip-received popup: a committed trip upload — which always lands *after* its member
    /// routes — replaces the last per-route popup of the burst with one "TRIP RECEIVED" card.
    /// Same family rules (advisory, 30 s auto-close, passkey outranks). Holds the trip's durable
    /// id, not a catalog index, so no rescan remap is needed.
    TripReceived(TripReceivedScreen) => Caps::modal().timed(),
    /// The BLE pairing passkey card (epic #447, P2). **Host-pushed** by [`App::set_ble_status`]
    /// when the seam's passkey goes `Some`, popped when it clears. Opaque + non-dismissible.
    Passkey(PasskeyScreen) => Caps::modal().blocking(),
    /// The map-transfer card (issue #927). **Host-pushed** by [`App::set_map_transfer`] when a map
    /// upload starts and popped when the state clears — the one screen a multi-minute SD write is
    /// visible on. Non-dismissible while bytes land, dismissable once terminal.
    MapTransfer(MapTransferScreen) => Caps::modal().blocking(),
    /// The advisory warning card (issue #504): missing sensors / a slow (fragmented) map.
    /// **Host-pushed** by the pass's fact stage, coalesced, dismissed on any press.
    Warning(WarningScreen) => Caps::modal(),
    /// The Weather dashboard (WX11, epic #1185): the concept-C decision card, the two-hour strip,
    /// and the HOURLY / RAIN MAP actions. Timed: the countdown/freshness copy moves once a minute.
    Weather(WeatherScreen) => Caps::nav().timed().key(RenderKeyKind::Weather),
    /// The hourly forecast list (WX11): 24 evenly-spaced rows, no separators — time, WX17 icon,
    /// temperature, precipitation, wind.
    WeatherHourly(WeatherHourlyScreen) => Caps::nav().key(RenderKeyKind::Weather),
    /// The rain map (WX11): the normal map scene with the WX10 precipitation raster below the
    /// road band, 15-minute time-step navigation, and the honest out-of-regime/stale banners. The
    /// **one** screen that declares [`rain_overlay`](Caps::rain_overlay) — the raster is its
    /// content, so it cannot survive the screen.
    WeatherRainMap(WeatherRainMapScreen) => Caps::map().timed().rain_overlay(),
    /// The weather alert card (WX11): RAIN AHEAD / STORM AHEAD with VIEW RAIN MAP + DISMISS.
    /// **Host-pushed** by [`App::show_weather_alert`]; alert *generation* is WX12's.
    WeatherAlert(WeatherAlertScreen) => Caps::modal(),
    Settings(SettingsScreen) => Caps::settings(),
    /// The Ride settings screen: routing profile + the riding stats grid (page cycle, fields, climb,
    /// waypoints) + the synced-ride retention ring. The one settings screen that scrolls (6 rows).
    Ride(RideScreen) => Caps::settings(),
    DateTime(DateTimeScreen) => Caps::settings(),
    Units(UnitsScreen) => Caps::settings(),
    /// The Bike type screen: cycles the routing profile (§8.6) the planner weights edges by, by name
    /// from the loaded map (routing-v2 N5, epic #533).
    BikeType(BikeTypeScreen) => Caps::settings(),
    StatFields(StatFieldsScreen) => Caps::settings().hold_fill(),
    AddField(AddFieldScreen) => Caps::settings(),
    /// The Display screen: the Map's clock + scale-bar overlay toggles and the idle-return timeout.
    Display(DisplayScreen) => Caps::settings(),
    /// The Connections settings menu: Phone (Bluetooth pairing) + Sensors (BLE sensors scan).
    Connections(ConnectionsScreen) => Caps::settings(),
    Power(PowerScreen) => Caps::settings(),
    /// The Bluetooth screen: radio on/off, status line, Paired row, hold-guarded Forget phone.
    Bluetooth(BluetoothScreen) => Caps::settings().hold_fill(),
    /// The Sensors screen (BLE sensors epic #707, SE7): the HR / power / cadence rows with their live
    /// status; press → scan list, hold a saved row → forget.
    Sensors(SensorsScreen) => Caps::settings().hold_fill().key(RenderKeyKind::SensorSettings),
    /// One quantity's live scan list (SE7): the discovered sensors of that kind; press saves + connects.
    SensorScan(SensorScanScreen) => Caps::settings().key(RenderKeyKind::SensorSettings),
    /// The Language screen (epic #602): cycles the UI language by endonym. Persists the choice today;
    /// the translation catalog that reads it lands later in the epic.
    Language(LanguageScreen) => Caps::settings(),
    /// The Weather settings screen (WX11): the scheduled refresh interval picker
    /// (Off / 15 / 30 / 60 / 120 min, default 30) the WX8 due scheduler consumes.
    WeatherSettings(WeatherSettingsScreen) => Caps::settings(),
    /// The System settings menu: Units / Date & Time / Language / Firmware update / About / Reset —
    /// a thin nav list whose rows open those pages.
    System(SystemScreen) => Caps::settings(),
    /// The Firmware page (epic #615 S5): the device-info ledger + the "Install update from card"
    /// door into the SD-sideload firmware-update flow.
    Firmware(FirmwareScreen) => Caps::settings(),
    /// The About page (issue #1149): the device's credits surface — OpenStreetMap + ODbL,
    /// Copernicus, and the firmware's GPL-3.0 + source pointer. A line-scrolling read-only page.
    About(AboutScreen) => Caps::settings(),
    Reset(ResetScreen) => Caps::settings().hold_fill(),
    /// The "Checking card..." scan wait (epic #615 S5): a spinner up while the board validates
    /// `UPDATE.BIN`; the board's answer replaces it with the confirm screen or an error card.
    DfuCheck(DfuCheckScreen) => Caps::modal().timed(),
    /// The install confirm (epic #615 S5): installed → update versions, the no-undo / same-version
    /// warnings, and the standard two-row Install / Cancel chrome.
    DfuConfirm(DfuConfirmScreen) => Caps::modal(),
    /// The "Preparing update..." progress spinner (epic #615 S5): up while the install one-shot
    /// waits for the board's drain; the drain swaps it for the terminal DfuInstalling card.
    DfuProgress(DfuProgressScreen) => Caps::modal().timed(),
    /// The static, terminal "Installing update" card: board-pushed right before the arm's warm
    /// reset — the last painted frame, which the MIP panel holds through the whole install.
    DfuInstalling(DfuInstallingScreen) => Caps::modal().blocking(),
    /// The scan-error card (epic #615 S5): a typed [`DfuScanError`](crate::dfu::DfuScanError) as a
    /// plain sentence; Back dismisses.
    DfuError(DfuErrorScreen) => Caps::modal(),
    /// The one-time "Updated to vX" post-update toast (epic #615 S5), host-pushed on the first
    /// healthy boot after an update.
    DfuUpdated(DfuUpdatedScreen) => Caps::modal(),
    /// The one-time "UPDATE FAILED" card, host-pushed by the boot-outcome reconcile on the first
    /// boot after an armed update that did not end with the staged image running (never started /
    /// reverted).
    DfuFailed(DfuFailedScreen) => Caps::modal(),
    /// The **universal quick drawer** (#1515 D2): the top sheet Up+Select opens from anywhere the
    /// chord is not suppressed. Four unlabelled device-wide controls — brightness, the BLE radio,
    /// central settings, power — plus the nested brightness editor and the guarded power
    /// confirmation. The first `Overlay` row: it composites over the base, which the frame then
    /// draws through the dim LUT, and its `Drawer` key freezes that base while it is up.
    QuickDrawer(QuickDrawerScreen) => Caps::overlay().hold_fill(),
    /// The **contextual drawer** (#1515 D3): the bottom sheet Down+Back opens on a screen that
    /// declares a [`ContextMenu`]. It holds no content of its own — the rows come from the base
    /// screen's [`context`](Screen::context) declaration — so one row here serves every context the
    /// D4 slices add.
    ContextDrawer(ContextDrawerScreen) => Caps::overlay(),
}

impl Screen {
    /// Whether this screen draws *over* the one below (the stack composites it on
    /// top) rather than replacing the view — derived from [`kind`](Screen::kind).
    pub fn is_overlay(&self) -> bool {
        self.kind().is_overlay()
    }

    /// **Pre-draw acquisition** (#803): resolve any reader-backed one-shot state before drawing, so
    /// [`draw`](Screen::draw) stays side-effect-free (target + render-stats only). Run on the base
    /// screen once per frame, ahead of the draw loop, whenever the host built the `Reader`
    /// ([`base_needs_reader`](crate::App::base_needs_reader) reads the same [`ReaderNeed`]
    /// declaration). The POI list takes its category snapshot into shared scratch, the detail
    /// resolves its opening-hours cache, and Skip ahead resolves route geometry + its live anchor;
    /// every other screen is a no-op.
    /// Intentionally partial, like [`tick_timers`](Screen::tick_timers) and
    /// [`wants_hold_fill`](Screen::wants_hold_fill): a row that declares no reader need never lands
    /// here.
    /// The **route-corridor snapshot** this screen wants, if any (epic #946, U3) — the Up-ahead
    /// timeline's `(filter, anchor)` key, declared rather than queried. Read by
    /// [`reconcile_corridor`](crate::ui_runtime::UiRuntime::reconcile_corridor) whenever the stack
    /// settles, which is the whole arm/re-arm/disarm lifecycle. Intentionally partial: no other
    /// screen asks for one, so the App-owned scratch stays disarmed (and the query free) everywhere
    /// else — as does an Up-ahead list the rider scoped to **waypoints only** (U4), which declares
    /// no key at all.
    pub(crate) fn corridor_request(
        &self,
        scope: crate::corridor::UpAheadScope,
    ) -> Option<crate::corridor::CorridorKey> {
        match self {
            Screen::UpAhead(s) => s.corridor_key(scope),
            _ => None,
        }
    }

    /// The **contextual content** this screen declares (#1515 D3) — the rows the Down+Back sheet
    /// offers over it, or `None` when it has no secondary actions and the chord therefore does
    /// nothing. Data, never behaviour: [`ContextDrawerScreen`] owns the cursor, the dimming, the
    /// transitions and the drawing, so a screen joins the grammar by naming a table.
    ///
    /// Intentionally partial like [`corridor_request`](Screen::corridor_request): most screens
    /// declare nothing, and an empty sheet is exactly what the issue forbids.
    pub(crate) fn context(&self) -> Option<&'static ContextMenu> {
        match self {
            // The four riding views share one context — the ride's secondary actions do not change
            // because the rider switched which readout they are looking at.
            Screen::Map(_) | Screen::Statistics(_) | Screen::Climb(_) | Screen::RideControl(_) => {
                Some(&context_drawer::RIDE)
            }
            // The timeline's two scope controls (#1515 D4a) — the only home either of them has.
            Screen::UpAhead(_) => Some(&context_drawer::UP_AHEAD),
            _ => None,
        }
    }

    pub(crate) fn prepare(&mut self, px: &mut Prepare) {
        match self {
            Screen::PoiList(s) => s.prepare(px),
            Screen::PoiDetail(s) => s.prepare(px),
            Screen::Detour(s) => s.prepare(px),
            Screen::DetourPreview(s) => s.prepare(px),
            _ => {}
        }
    }

    /// Whether this screen's `draw` would fill a live hold bar for its **current** selection/state
    /// — the guarded confirm rows (Ride control, Route swap), the *armed* factory-Reset bar, the
    /// Fields hold-to-delete footer over a deletable row, and the Route overview's Delete-route row
    /// over a deletable route. A render-on-demand host uses
    /// [`App::top_wants_hold_fill`](crate::App::top_wants_hold_fill) to repaint a charging hold
    /// only when the fill would actually draw. Intentionally partial, like
    /// [`tick_timers`](Screen::tick_timers): most screens draw nothing hold-driven.
    pub(crate) fn wants_hold_fill(
        &self,
        settings: &Settings,
        state: &crate::AppState,
        activity: &Activity,
        recording: bool,
        routes: &[RouteSummary],
        rides: &[RideSummary],
    ) -> bool {
        match self {
            Screen::RideControl(s) => s.selection_is_guarded(),
            Screen::RideRecovery(s) => s.selection_is_guarded(),
            Screen::RouteSwap(s) => s.selection_is_guarded(),
            Screen::Reset(s) => s.hold_fill_active(),
            Screen::StatFields(s) => s.selection_is_deletable(settings),
            Screen::Bluetooth(s) => s.selection_is_guarded(state.device.ble_paired),
            Screen::QuickDrawer(s) => s.selection_is_guarded(),
            Screen::Sensors(s) => s.selection_is_guarded(settings),
            Screen::RouteOverview(s) => s.selection_is_guarded(activity, recording, routes),
            Screen::RideDetail(s) => s.selection_is_guarded(recording, rides.len()),
            Screen::TripDelete(s) => s.selection_is_guarded(),
            _ => false,
        }
    }

    /// Poll this screen's time-driven content one frame: fire any timed change that is due and
    /// report the residual deadline to the next one, both computed from the same gating locals so
    /// "did it change" and "when next" can never drift apart. [`ScreenTick::changed`] is how the
    /// render-on-demand host marks the map dirty (issue #47); [`ScreenTick::next_wake_ms`] is what
    /// the event-driven host (issue #219) folds across the visible stack into a single wake
    /// deadline so the M33 sleeps rather than free-running the loop.
    ///
    /// Most screens change only on input or a fresh fix and return [`ScreenTick::idle`]. The
    /// Statistics view runs its stat-grid page auto-cycle off `now_ms`; the Home clock
    /// ticks over each minute off the wall-clock `now`, adopting `ms_to_next_minute` — the minute
    /// boundary the host pre-computes (it owns the clock); the Menu sweeps its compass needle
    /// toward the selection at frame cadence until it lands.
    /// `w`/`h` are the panel size in device pixels (the last rendered frame's — see
    /// [`App::advance_animations`](crate::App::advance_animations)), for the screens that report a
    /// dirty [`region`](ScreenTick::region); `0` before the first frame, which makes them abstain.
    /// `pan_active` lets the Map gate its clock overlay (the pan chevron owns the top slot).
    #[allow(clippy::too_many_arguments)] // one poll fn threading every timed screen's inputs
    pub fn tick_timers(
        &mut self,
        now_ms: u32,
        now: DateTime,
        ms_to_next_minute: u32,
        settings: &Settings,
        w: i32,
        h: i32,
        pan_active: bool,
        tracking: bool,
    ) -> ScreenTick {
        match self {
            // The drawer's sheet animation: the slide-down on open, the horizontal page slide, and
            // the height the sheet adapts to the page it lands on.
            Screen::QuickDrawer(s) => s.tick_timers(now_ms),
            // The context sheet's slide up from the bottom edge.
            Screen::ContextDrawer(s) => s.tick_timers(now_ms),
            Screen::Statistics(s) => s.tick_timers(now_ms, settings),
            Screen::Home(s) => s.tick_timers(now, ms_to_next_minute),
            // The Map's clock overlay ticks over each minute (region-clipped to the pill), armed only
            // when the pill is visible — the setting on and not panning (Inspect keeps the map free
            // of unrelated clock chrome);
            // it also runs the route-less browse map's one-shot start-hint timer (T6, gated on `tracking`).
            Screen::Map(s) => {
                s.tick_timers(now_ms, now, ms_to_next_minute, w, pan_active, settings.map_clock, tracking)
            }
            Screen::Menu(s) => s.tick_timers(now_ms),
            // The route-upload popups' 30 s auto-close deadline (epic #447, P4): the residual
            // wake keeps the event-driven host armed so the timeout-dismiss fires from warm
            // sleep; the removal itself runs in `App::advance_animations`' popup sweep.
            Screen::RouteReceived(s) => s.tick_timers(now_ms),
            Screen::RouteUpdated(s) => s.tick_timers(now_ms),
            Screen::TripReceived(s) => s.tick_timers(now_ms),
            Screen::RouteSwap(s) => s.tick_timers(now_ms),
            // The Route overview's content-paired pager (T3, re-paired in #678 rework 3): flips
            // track shape + DISTANCE ↔ elevation band + CLIMB + DESCENT every 5 s.
            Screen::RouteOverview(s) => s.tick_timers(now_ms),
            // The Ride detail's content-paired pager (owner review rounds 2 + 3): the same 5 s
            // flip, track shape + DISTANCE + RIDE TIME ↔ elevation band + AVG + CLIMBED.
            Screen::RideDetail(s) => s.tick_timers(now_ms),
            // The nav planning spinner (#499): free-runs at frame cadence until the host's
            // answer (or a cancel) removes the screen. The one screen that reports a dirty
            // region — the spinning needle's disc — so the multi-second plan's repaints stay
            // region-cheap (#500 follow-up).
            Screen::NavPlanning(s) => s.tick_timers(now_ms, w, h),
            // The DFU wait spinners (epic #615 S5): free-run at frame cadence, reporting the
            // needle disc as their dirty region like the nav planner, until the board's answer /
            // reboot replaces them.
            Screen::DfuCheck(s) => s.tick_timers(now_ms, w, h),
            Screen::DfuProgress(s) => s.tick_timers(now_ms, w, h),
            // The Weather dashboard's countdown + the rain map's frame-currency labels move with
            // the wall clock — one region-free repaint per minute while up.
            Screen::Weather(s) => s.tick_timers(now, ms_to_next_minute),
            Screen::WeatherRainMap(s) => s.tick_timers(now, ms_to_next_minute),
            _ => ScreenTick::idle(),
        }
    }
}

/// The result of one [`Screen::tick_timers`] poll: whether a timed change just fired (the host
/// repaints) and how long until the next one is due (the host arms its wake timer). Produced in
/// one body per screen, so the two halves of the timing contract share their gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenTick {
    /// A timed change fired this poll — the drawn output differs, so the map plane needs a repaint.
    pub changed: bool,
    /// Milliseconds until the next timed change is due, or `None` when no timer is pending (the
    /// screen changes only on input or a fresh fix). Strictly positive: a due timer fired this
    /// poll instead.
    pub next_wake_ms: Option<u32>,
    /// Where this poll's fired change is contained, in panel pixels — `None` means anywhere (the
    /// full-frame repaint every screen implies by default; no screen has to opt in). `Some(r)` is
    /// the screen's promise that the drawn output differs from the previous frame **only inside
    /// `r`**, so a host may clip the repaint to it (the region-scoped repaint, #500 follow-up —
    /// today only the nav-planning spinner's needle disc). Read only when
    /// [`changed`](ScreenTick::changed) fired.
    pub region: Option<Rectangle>,
}

impl ScreenTick {
    /// No timed content: nothing changed, nothing pending — the arm for every static screen.
    pub const fn idle() -> Self {
        ScreenTick { changed: false, next_wake_ms: None, region: None }
    }
}

/// How long a route-upload popup (epic #447, P4) stays up before it auto-closes; the timeout **is**
/// a dismiss — the popups are advisory (the route is committed before any prompt), so expiring
/// loses nothing. Long enough to read mid-ride, short enough that a parked device returns to warm
/// sleep on its own.
pub const UPLOAD_POPUP_TIMEOUT_MS: u32 = 30_000;

/// Start riding catalog route `i` from a non-tracking state — **the** route-start path: the riding
/// camera seeded on the route's start, [`Mode::Riding`], `active_route` pointed at it, a fresh
/// tracking session, and a clean `[Home, Map]` stack. Shared by the Route overview's START RIDE
/// press and the "ROUTE RECEIVED" popup's *Start navigation* (locked to be exactly this path), so
/// the two can never drift. An out-of-range `i` (the route vanished in a rescan) pops instead.
pub(crate) fn start_ride(cx: &mut Ctx, i: usize) -> Transition {
    let Some(route) = cx.routes.get(i) else {
        return Transition::Pop;
    };
    let (lon, lat) = (route.start_lon, route.start_lat);
    cx.activity.active_route = Some(i);
    begin_riding_session(cx, lon, lat)
}

/// Start a **route-less** tracking session from a non-tracking state — the browse Map's start card
/// (Menu → Map → press → *Start ride*). Identical to [`start_ride`] minus the route: no
/// `active_route`, and the riding camera seeded on the rider's last fix (or the current camera when
/// no fix yet) rather than a route start. The recorded ride saves and behaves exactly like a
/// route-guided one (same session, ORD file, breadcrumb, BLE ride object) — only with no route to
/// navigate against, so the route-relative stats read `--`.
pub(crate) fn start_ride_routeless(cx: &mut Ctx) -> Transition {
    // Seed the camera where the rider is (the last fix), falling back to the current camera so the
    // first frame is sensible before any fix. Follow mode recenters on each fix regardless.
    let (lon, lat) = cx.state.user_fix.map_or((cx.state.cam_lon, cx.state.cam_lat), |f| (f.lon, f.lat));
    cx.activity.active_route = None;
    begin_riding_session(cx, lon, lat)
}

/// The session-begin shared by [`start_ride`] and [`start_ride_routeless`]: enter the riding view
/// (camera seeded at `(lon, lat)`), go [`Mode::Riding`], open a fresh tracking session, and root the
/// stack to a clean `[Home, Map]`. The caller sets `active_route` first (a route index, or `None`
/// for a route-less ride) — the one thing that differs between the two starts.
fn begin_riding_session(cx: &mut Ctx, lon: i32, lat: i32) -> Transition {
    cx.state.enter_riding_view(lon, lat);
    cx.activity.mode = Mode::Riding;
    cx.recorder.request(crate::RecorderIntent::Start);
    Transition::Root(Screen::Map(MapScreen::new()))
}

/// The one gesture the riding views bind identically: `press` pauses tracking and opens the
/// Ride-control page. Each riding screen calls this from its `Press` arm.
///
/// Its Back-hold arm is gone with the compass ride menu (#1515 D3): Back-hold is the global escape
/// now, resolved in [`App`](crate::App) above screen dispatch, and the ride's secondary actions
/// live in the [contextual drawer](context_drawer) the four riding views declare.
pub(crate) fn riding_common(g: Gesture, cx: &mut Ctx) -> Transition {
    match g {
        Gesture::Press => {
            cx.activity.mode = Mode::Paused;
            Transition::Push(Screen::RideControl(RideControl::new()))
        }
        _ => Transition::None,
    }
}

/// The "explorer's field map" palette in RGB565, so screen text and chrome quantize through the
/// host `color_fn` exactly like map styles.
///
/// Tuned to the 64-color (RGB222) gamut: the panel has 4 levels per channel (0/85/170/255), so each
/// value is chosen for the *quantized* result. The trailing comment on each is the device-64 RGB it
/// lands on; `tests/palette.rs` asserts every one through `rgb565_to_device64`, so a retune that
/// forgets to update a comment fails the build.
pub mod palette {
    /// Pack 8-bit RGB into RGB565.
    pub const fn rgb565(r: u8, g: u8, b: u8) -> u16 {
        (((r as u16) >> 3) << 11) | (((g as u16) >> 2) << 5) | ((b as u16) >> 3)
    }

    // Device-64 has no warm off-white — any blue < 192 tints it yellow — so this is a clean
    // near-white; the wood frame + ink + amber carry the warmth instead.
    pub const PARCHMENT: u16 = rgb565(245, 243, 238); // → (255,255,255) white
    pub const PARCHMENT_SHADE: u16 = rgb565(180, 170, 105); // → (170,170,85) tan
    pub const HUD: u16 = rgb565(46, 37, 26); // → (0,0,0) near-black frame
    pub const WOOD: u16 = rgb565(150, 100, 40); // → (170,85,0) wood brown
    /// Lighter wood for inset borders / frame lines.
    pub const WOOD_LIGHT: u16 = rgb565(180, 168, 100); // → (170,170,85) tan
    pub const INK: u16 = rgb565(44, 33, 20); // → (0,0,0) text black
    /// Muted ink for secondary / sub-label text.
    pub const SUBTEXT: u16 = rgb565(110, 90, 58); // → (85,85,0) olive
    /// Hairline rule between list rows.
    pub const RULE: u16 = rgb565(180, 170, 100); // → (170,170,85) tan
    pub const AMBER: u16 = rgb565(227, 165, 43); // → (255,170,0) accent
    pub const WARNING: u16 = rgb565(192, 73, 46); // → (255,85,0) warning
    /// Faint neutral grey — the Home screensaver's contour lines and empty battery cells: dim
    /// enough to sit behind the clock, bright enough to read as fine topo lines.
    pub const CONTOUR: u16 = rgb565(96, 96, 96); // → (85,85,85) grey
    /// Green — the "on" state of a settings toggle pill (ink = off), and the shallowest band of the
    /// Climb screen's grade ramp (`< 3 %`). The only green on the panel.
    pub const ON: u16 = rgb565(0, 170, 0); // → (0,170,0) green
    /// Yellow — the Climb screen's `3–6 %` grade band. Between [`ON`] green and [`AMBER`] on the
    /// ClimbPro ramp; device-64 has a pure `(255,255,0)`, so it reads distinctly from amber.
    pub const YELLOW: u16 = rgb565(255, 255, 0); // → (255,255,0) yellow
    /// Red — the Climb screen's steepest grade band (`> 12 %`). The panel's pure red; hotter than the
    /// [`WARNING`] orange so the two never blur into one another on the stripes.
    pub const RED: u16 = rgb565(255, 0, 0); // → (255,0,0) red
    /// Apricot — the Climb screen's tile background. Warmer + lighter than Statistics' tan
    /// [`PARCHMENT_SHADE`], so the two riding views' grids read apart at a glance (decided with the
    /// user). Device-64 `(255,170,85)`.
    pub const CLIMB_TILE: u16 = rgb565(255, 170, 85); // → (255,170,85) apricot
    /// Magenta — the planned route line on the Map. The classic GPS route hue: it lands on no
    /// base-map feature, so it always reads as "the line to follow".
    pub const ROUTE: u16 = rgb565(255, 0, 255); // → (255,0,255) magenta
    /// Blue — the planned detour's polyline on the Detour preview (#882): the replanned portion
    /// reads apart from the magenta route it will replace, the warning-orange skipped span, and
    /// the (recessive navy) breadcrumb behind it.
    pub const DETOUR: u16 = rgb565(0, 90, 255); // → (0,85,255) blue
    /// Rain blue — the precipitation *amount* on the Hourly rows, so a wet hour's millimetres read
    /// as water at a glance rather than as another ink number. The WX17 icons' own rain-streak
    /// blue (`weather_icons::SKY`), so the row's icon and its number carry one hue.
    pub const RAIN: u16 = rgb565(0, 110, 230); // → (0,85,255) blue
    /// Navy — the recorded breadcrumb (travelled path), stroked over the route and under the marker.
    /// Recessive so the trail behind reads quieter than the magenta route ahead.
    pub const BREADCRUMB: u16 = rgb565(0, 0, 170); // → (0,0,170) navy
}

/// One RGB222 channel level, stepped down. Index by the channel's stored level (0-3); the result
/// is another level, so the dimmed colour stays exactly on the device gamut and nothing has to be
/// re-quantized. Four bytes in `.rodata`, carried over from the prototype's tuned captures.
const DIM_LEVEL: [u8; 4] = [0, 1, 1, 2];

/// The **dim policy** a frame draws its base through while a drawer covers it: an RGB565 colour in,
/// the same colour one device-64 level darker out.
///
/// This is a colour function, not a layer. [`App::draw_frame`](crate::App) composes it with the
/// host's own `color_fn` for the base screen and hands the sheet the untouched `color_fn`, and it
/// costs **zero** bytes of RAM - no capture buffer, no second framebuffer, no alpha, and nothing
/// for the 64-colour panel to approximate.
///
/// **Per colour resolution, not per pixel.** `Canvas` resolves `color_fn` once per primitive - one
/// span, one outline, one string, one sampled rain cell - so this runs O(primitives), not
/// O(pixels), and the MIP's partial-line budget never sees it.
pub(crate) fn dim_color(rgb565: u16) -> u16 {
    let r = DIM_LEVEL[((rgb565 >> 14) & 0x3) as usize];
    let g = DIM_LEVEL[((rgb565 >> 9) & 0x3) as usize];
    let b = DIM_LEVEL[((rgb565 >> 3) & 0x3) as usize];
    palette::rgb565(r * 85, g * 85, b * 85)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dim LUT never brightens a channel, and lands the two extremes where the prototype's
    /// captures did: parchment recedes to a mid grey, and the near-black chrome stays black.
    #[test]
    fn the_dim_lut_only_darkens_and_stays_on_the_gamut() {
        for level in 0..4u16 {
            for shift in [14, 9, 3] {
                let dimmed = dim_color(level << shift);
                assert!((dimmed >> shift) & 0x3 <= level, "channel at bit {shift} brightened");
            }
        }
        assert_eq!(dim_color(palette::PARCHMENT), palette::rgb565(170, 170, 170), "parchment recedes to grey");
        assert_eq!(dim_color(palette::HUD), palette::rgb565(0, 0, 0), "the darkest chrome has nowhere to go");
    }

    /// `Screen::NAMES` is a usable drift-guard key set: every name unique and non-empty, and the
    /// table agrees with [`Screen::name`] (both are generated from the one `screens!` table, so
    /// this pins the macro plumbing, not a hand-kept list).
    #[test]
    fn screen_names_are_unique_and_match_name() {
        assert!(!Screen::NAMES.is_empty());
        for (i, n) in Screen::NAMES.iter().enumerate() {
            assert!(!n.is_empty());
            assert!(!Screen::NAMES[..i].contains(n), "duplicate screen name {n}");
        }
        assert_eq!(Screen::Home(HomeScreen::new()).name(), "Home");
        assert!(Screen::NAMES.contains(&"Home") && Screen::NAMES.contains(&"Map"));
    }

    // ── Capability metadata (#803) ──────────────────────────────────────────────────────────────

    /// The [`Caps`] table and the [`NAMES`](Screen::NAMES) table are generated from the same
    /// `screens!` rows, so they must stay index-for-index aligned — the enumeration the capability
    /// invariants below iterate.
    #[test]
    fn caps_table_pairs_with_names() {
        assert_eq!(Screen::CAPS.len(), Screen::NAMES.len(), "one Caps per screen name");
        assert!(!Screen::CAPS.is_empty());
    }

    /// The generated per-variant [`caps`](Screen::caps) match agrees with the [`CAPS`](Screen::CAPS)
    /// const table at each variant's index — pins the macro plumbing (both come from the one table,
    /// like [`name`](Screen::name) vs [`NAMES`](Screen::NAMES)).
    #[test]
    fn constructed_caps_match_the_table() {
        for (name, scr) in [
            ("Home", Screen::Home(HomeScreen::new())),
            ("Map", Screen::Map(MapScreen::new())),
            ("Statistics", Screen::Statistics(StatisticsScreen::new())),
            ("Menu", Screen::Menu(MenuScreen::new())),
            ("PoiList", Screen::PoiList(PoiListScreen::new(obc_reader::PoiCategory::Water))),
        ] {
            let idx = Screen::NAMES.iter().position(|n| *n == name).unwrap();
            assert_eq!(scr.caps(), Screen::CAPS[idx], "{name}.caps() must equal CAPS[{idx}]");
            assert_eq!(scr.kind(), Screen::CAPS[idx].kind, "{name}.kind() derives from its Caps");
        }
    }

    /// Every screen's declared capabilities are internally consistent — the acceptance-criterion
    /// invariants. A screen that declares one capability must declare the companions it implies, so
    /// a mis-declared row fails here instead of silently mis-routing a policy at runtime.
    #[test]
    fn every_screen_capability_combination_is_valid() {
        for (name, c) in Screen::NAMES.iter().zip(Screen::CAPS) {
            // Reader need is pinned to base content: map bases are always-reader screens, and the
            // two POI one-shot readers are chrome-kind list/detail screens.
            match c.reader {
                ReaderNeed::Always => assert_eq!(c.base, BaseContent::Map, "{name}: Always-reader ⟺ Map base"),
                ReaderNeed::Never => assert_ne!(c.base, BaseContent::Map, "{name}: a Map base must read Always"),
                ReaderNeed::PoiSnapshot | ReaderNeed::PoiHours => {
                    assert_eq!(c.base, BaseContent::Chrome, "{name}: a POI reader screen is chrome-based");
                    assert_eq!(c.kind, ScreenKind::Nav, "{name}: a POI reader screen is Nav-kind");
                }
            }
            // A non-chrome base (Map / LiveRiding) is a live view fed by the fix — and a deliberate
            // ride view (never idle-returned mid-ride).
            if c.base != BaseContent::Chrome {
                assert!(c.ride_view, "{name}: a live-data base must be a ride view");
                assert!(!c.idle_exempt, "{name}: a live view is not a modal exemption");
            }
            // A rain-overlay screen must be a map base: the raster draws inside the map scene's
            // paint order, so there is nowhere for it to go on a chrome or live-riding screen. And
            // it must not be an *overlay* kind: the lease is resolved against the base (lowest
            // non-overlay) screen, so a rain screen declared `Overlay` would carry a capability
            // that never fires — a silently dead declaration, exactly the drift this table exists
            // to catch.
            if c.rain_overlay {
                assert_eq!(c.base, BaseContent::Map, "{name}: only a Map base can carry the rain overlay");
                assert!(
                    !c.kind.is_overlay(),
                    "{name}: an overlay-kind screen is never the base the lease resolves against"
                );
            }
            // A browse-exempt "deliberate view when not tracking" must be map-based.
            if c.browse_exempt {
                assert_eq!(c.base, BaseContent::Map, "{name}: only a Map base is browse-exempt");
            }
            // Modal exemptions are chrome cards/waits, never ride views.
            if c.idle_exempt {
                assert_eq!(c.base, BaseContent::Chrome, "{name}: an idle-exempt modal is chrome-based");
                assert!(!c.ride_view, "{name}: an idle-exempt modal is not a ride view");
            }
            // A settings-subtree screen is pure chrome with no live/idle/reader/remap role.
            if c.kind == ScreenKind::Settings {
                assert_eq!(c.base, BaseContent::Chrome, "{name}: a settings screen is chrome-based");
                assert_eq!(c.reader, ReaderNeed::Never, "{name}: a settings screen needs no reader");
                assert_eq!(c.remap, RemapKind::None, "{name}: a settings screen holds no catalog index");
                assert!(!c.ride_view && !c.idle_exempt && !c.browse_exempt, "{name}: settings carry no view policy");
            }
            // Ride-catalog holders are chrome list/detail screens. Route holders also include the
            // live map-backed Skip chooser, so route remapping deliberately has no base restriction.
            if c.remap == RemapKind::Ride {
                assert_eq!(c.base, BaseContent::Chrome, "{name}: a ride-remap screen is chrome-based");
            }
            // A drawer is an overlay sheet, and the two halves of that are one declaration: the
            // frame dims the base under any `Overlay` row, and the pass freezes it because that
            // row's key kind shadows the base's. Declaring one without the other would either dim
            // a base that keeps repainting or freeze one that never recedes.
            assert_eq!(
                c.kind.is_overlay(),
                c.render_key == RenderKeyKind::Drawer,
                "{name}: the Drawer key kind and the Overlay screen kind are one declaration"
            );
            if c.kind.is_overlay() {
                assert_eq!(c.base, BaseContent::Chrome, "{name}: a sheet draws chrome over the base it covers");
                assert!(c.timed, "{name}: a sheet animates, so it needs a tick arm");
                assert!(!c.idle_exempt, "{name}: a drawer is not a modal the idle return must respect");
                assert!(!c.blocks_chords, "{name}: a drawer must not suppress the chord that closes it");
            }
            // Only a genuinely blocking modal may refuse the device-wide chords, and anything
            // that refuses a chord refuses the global escape too — the escape's set is the wider.
            if c.blocks_chords {
                assert!(c.idle_exempt, "{name}: only an idle-exempt modal is blocking enough to refuse a chord");
                assert!(c.blocks_escape, "{name}: a screen that refuses a chord must refuse the escape too");
            }
            if c.blocks_escape {
                assert!(c.idle_exempt, "{name}: only a modal the rider must answer may refuse the escape");
            }
        }
    }

    /// Every declared capability is actually exercised by at least one screen, and the headline
    /// classifications land on the screens they should — a coarse guard that the table isn't
    /// mis-populated (e.g. every reader kind, both remap catalogs, and the ride-view/modal roles
    /// have a member).
    #[test]
    fn capability_coverage_and_landmarks() {
        let caps = Screen::CAPS;
        let named = |name: &str| caps[Screen::NAMES.iter().position(|n| *n == name).unwrap()];
        // Landmark screens carry the capabilities their behavior depends on.
        assert_eq!(named("Map").base, BaseContent::Map);
        assert_eq!(named("Map").reader, ReaderNeed::Always);
        assert!(named("Map").browse_exempt && named("Map").ride_view && named("Map").timed);
        assert_eq!(named("Statistics").base, BaseContent::LiveRiding);
        assert_eq!(named("Climb").base, BaseContent::LiveRiding);
        assert_eq!(named("PoiList").reader, ReaderNeed::PoiSnapshot);
        assert_eq!(named("PoiDetail").reader, ReaderNeed::PoiHours);
        assert!(named("RideControl").ride_view, "the Paused page is a deliberate ride view");
        assert!(named("Passkey").idle_exempt, "the passkey card is idle-exempt");
        assert!(named("RouteSwap").idle_exempt, "the route-swap prompt is idle-exempt");
        assert_eq!(named("RouteMenu").remap, RemapKind::Route);
        assert_eq!(named("Detour").remap, RemapKind::Route);
        assert_eq!(named("DetourPreview").remap, RemapKind::Route);
        assert_eq!(named("Rides").remap, RemapKind::Ride);
        // The rain overlay belongs to the rain map and to nothing else — the Map and the Detour
        // pair draw the very same scene through `draw_map_scene`, so a stray `.rain_overlay()` on
        // one of them is exactly how the raster would start outliving its screen again.
        // The suppression set, named: exactly the three screens a squeeze must not reach past.
        let blocking: std::vec::Vec<&str> =
            Screen::NAMES.iter().zip(caps).filter(|(_, c)| c.blocks_chords).map(|(n, _)| *n).collect();
        assert_eq!(blocking, ["Passkey", "MapTransfer", "DfuInstalling"], "the chord suppression set, in table order");
        // The escape's set is the chord's plus the recovered-ride card: a sheet over it is
        // harmless, but leaving it would strand the recovered recording.
        let no_escape: std::vec::Vec<&str> =
            Screen::NAMES.iter().zip(caps).filter(|(_, c)| c.blocks_escape).map(|(n, _)| *n).collect();
        assert_eq!(
            no_escape,
            ["RideRecovery", "Passkey", "MapTransfer", "DfuInstalling"],
            "the escape suppression set, in table order"
        );
        assert_eq!(caps.iter().filter(|c| c.kind.is_overlay()).count(), 2, "the two drawers");
        assert!(named("QuickDrawer").kind.is_overlay() && named("QuickDrawer").hold_fill);
        assert!(named("ContextDrawer").kind.is_overlay());
        assert!(named("WeatherRainMap").rain_overlay, "the rain map is the screen rain belongs to");
        assert!(!named("Map").rain_overlay, "the ordinary Map never draws rain");
        assert_eq!(caps.iter().filter(|c| c.rain_overlay).count(), 1, "exactly one screen wants rain");
        // Each capability value is used by at least one screen (nothing dead-declared).
        assert!(caps.iter().any(|c| c.reader == ReaderNeed::Always));
        assert!(caps.iter().any(|c| c.reader == ReaderNeed::PoiSnapshot));
        assert!(caps.iter().any(|c| c.reader == ReaderNeed::PoiHours));
        assert!(caps.iter().any(|c| c.remap == RemapKind::Route));
        assert!(caps.iter().any(|c| c.remap == RemapKind::Ride));
        assert!(caps.iter().any(|c| c.timed));
        assert!(caps.iter().any(|c| c.hold_fill));
        assert!(caps.iter().any(|c| c.idle_exempt));
        assert!(caps.iter().any(|c| c.base == BaseContent::Map));
        assert!(caps.iter().any(|c| c.base == BaseContent::LiveRiding));
        assert!(caps.iter().any(|c| c.base == BaseContent::Chrome));
    }

    /// The capability additions compile to `const` tables and generated matches, never to fields on
    /// the enum — so `size_of::<Screen>()` (and thus every `.bss` screen-stack slot) is unchanged.
    /// The board's resident-RAM guard is the ELF authority; this pins the host measurement (the
    /// pre-#803 baseline: 104 B on the 64-bit host) so a variant-widening regression fails in CI.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn screen_enum_size_is_unchanged() {
        assert_eq!(core::mem::size_of::<Screen>(), 104, "capability metadata must not inflate the Screen enum");
    }
}
