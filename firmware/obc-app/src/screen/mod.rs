//! The screen system: `no_std`, zero-alloc, no retained widget tree. Screens are a [`Screen`] enum
//! dispatched by `match`, each variant a small module with typed state. Navigation is a return
//! value: [`handle`](Screen::handle) returns a [`Transition`] that [`apply`] runs against the stack.
//!
//! The shared context is split by role: [`Ctx`] is the logic half handed to `handle`, [`Render`] is
//! the draw half. This module holds the navigation engine only; the drawing vocabulary every screen
//! composes its page from lives one module per concept under [`vocab`].

use core::ops::{Deref, DerefMut};

use embedded_graphics::{draw_target::DrawTarget, primitives::Rectangle};
use obc_ports::Fix;
use obc_reader::Reader;
use obc_render::{Canvas, Clock, RenderScratch, RenderStats};
use obc_route::{ClimbProfile, ClimbSeg, Profile, RouteReader, Waypoints};

use crate::activity::{Activity, Mode};
use crate::app::AppState;
use crate::breadcrumb::Breadcrumb;
use crate::input::Gesture;
use crate::ride::RideEntry;
use crate::route::RouteSummary;
use crate::settings::{DateTime, Settings};

mod arrival;
pub(crate) mod assistant;
mod climb;
pub(crate) mod context_drawer;
mod detour;
mod dfu;
mod find_place;
mod home;
mod journey;
mod landmark_photo;
mod landmarks;
pub(crate) mod map;
mod map_transfer;
mod menu;
mod nav_route;
mod passkey;
mod peak_article;
mod peak_view;
mod poi_detail;
pub(crate) mod poi_display;
mod poi_list;
pub(crate) mod poi_menu;
mod quick_drawer;
mod ride_control;
mod ride_detail;
mod ride_recovery;
mod ride_start;
mod rides;
mod route_cleanup;
mod route_menu;
mod route_overview;
mod route_received;
mod route_swap;
pub(crate) mod settings;
mod start_away;
mod statistics;
mod trip_delete;
pub(crate) mod vocab;
mod warning;

pub use arrival::ArrivalScreen;
pub(crate) use arrival::ArrivalView;
pub use assistant::AssistantScreen;
pub use climb::ClimbScreen;
pub(crate) use context_drawer::ContextFacts;
pub use context_drawer::{ContextDrawerScreen, ContextMenu, ContextValue};
pub use detour::{DetourPreviewScreen, DetourScreen};
pub use dfu::{
    DfuCheckScreen, DfuConfirmScreen, DfuErrorReason, DfuErrorScreen, DfuFailedScreen, DfuInstallingScreen,
    DfuProgressScreen, DfuUpdatedScreen,
};
pub use find_place::{FindPlaceScreen, VisitReviewScreen};
pub use home::HomeScreen;
pub(crate) use journey::JourneyError;
pub use journey::JourneyScreen;
pub use landmark_photo::LandmarkPhotoScreen;
pub use landmarks::{LandmarkSourcesScreen, LandmarksScreen};
pub(crate) use map::low_battery_cue;
pub use map::{MapScreen, ROUTE_WEIGHT};
pub use map_transfer::{MapTransfer, MapTransferError, MapTransferScreen};
pub use menu::MenuScreen;
pub use nav_route::{NavFailScreen, NavPlanningScreen, PlanKind};
pub use passkey::PasskeyScreen;
pub use peak_article::PeakArticleScreen;
pub use peak_view::PeakViewScreen;
pub use poi_detail::PoiDetailScreen;
mod easier;
pub use easier::EasierScreen;
pub(crate) use poi_display::poi_row_name;
pub use poi_list::{PoiListScreen, PoiScratch};
/// The quick drawer's open duration, so the in-crate harness can settle a sheet before it acts on
/// one and a retune cannot leave that helper acting mid-slide.
#[cfg(test)]
pub(crate) use quick_drawer::OPEN_MS as QUICK_OPEN_MS;
pub use quick_drawer::{QuickDrawerScreen, BRIGHTNESS_LEVELS, BRIGHTNESS_MAX};
pub use ride_control::RideControl;
pub use ride_detail::RideDetailScreen;
pub use ride_recovery::{RecoveryMode, RideRecoveryScreen};
pub use ride_start::RideStartScreen;
pub use rides::RidesScreen;
pub use route_cleanup::RouteCleanupScreen;
pub use route_menu::RouteMenuScreen;
pub use route_overview::RouteOverviewScreen;
pub use route_received::{RouteReceivedScreen, RouteUpdatedScreen, TripReceivedScreen};
pub use route_swap::RouteSwapScreen;
pub use settings::{
    AboutScreen, AddFieldScreen, LanguageScreen, ResetScreen, SensorScanScreen, SensorsScreen, SettingsPage,
    StatFieldsScreen,
};
pub use start_away::StartAwayScreen;
pub use statistics::StatisticsScreen;
pub use trip_delete::TripDeleteScreen;
mod whats_next;
pub use poi_display::OFF_ROUTE_HINT_M;
/// The wait spinner's dirty disc is part of the host-facing repaint contract
/// (`ScreenTick::region`), so it is re-exported for the integration tests that pin it. In-crate
/// callers still import `vocab::spinner`.
pub use vocab::spinner::needle_region;
pub use warning::{WarningFlags, WarningScreen};
pub use whats_next::WhatsNextScreen;

/// Maximum overlay depth. The deepest normal path is seven screens
/// (`Home → Map → Menu → Settings → Ride → Fields → Add field`); the rest of the slots are for
/// host-pushed cards that arrive while such a path is open.
pub const MAX_DEPTH: usize = 10;

/// The screen stack: the bottom is the always-present root (Home), the top is the
/// screen currently receiving input.
pub type Stack = heapless::Vec<Screen, MAX_DEPTH>;

/// What a screen's [`handle`](Screen::handle) asks the navigation stack to do next; [`apply`] runs it.
pub enum Transition {
    None,
    Push(Screen),
    Pop,
    /// Swap this screen for `screen` without growing the stack.
    Replace(Screen),
    /// Drop the descent back to the root pair — the Home root and the view it stands on, what an
    /// escape leaves under its Menu — and push `screen` there.
    ///
    /// It is the transition for an arrival that is reached from anywhere — a row on a sheet, or a
    /// device-wide chord. Both open over the way down they themselves open, so an arrival that
    /// kept the descent would lay one way down on another and spend a slot every lap.
    OverRoot(Screen),
    /// Truncate to the Home root and push `screen`, landing on a clean `[Home, screen]` from any
    /// depth rather than leaving stale screens buried under the new one.
    Root(Screen),
    /// Clear every overlay back to the Home root.
    Home,
}

/// Whether the device is on the terminal powering-off frame: the rider completed the guarded hold
/// and the host is about to call the power-off port. Nothing dismisses it. A stack fact, because
/// the operations that must respect it are stack operations.
pub(crate) fn powering_off(stack: &Stack) -> bool {
    matches!(stack.last(), Some(Screen::QuickDrawer(d)) if d.powering_off())
}

/// The index of the lowest opaque screen: where the frame starts, because everything under it is
/// covered. Drawing, ticking, the pre-draw acquisition and the render key all begin there, so they
/// cannot disagree about which screen is the base. `0` when nothing on the stack is opaque.
pub(crate) fn base_index(stack: &Stack) -> usize {
    stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0)
}

/// The lowest opaque screen itself: the frame's base. `None` only for an empty stack, which the
/// root forbids.
pub(crate) fn base_screen(stack: &Stack) -> Option<&Screen> {
    stack.get(base_index(stack))
}

/// Nothing lands on top of a drawer: take any open sheet off the top of `stack`, and report whether
/// one was there. Called wherever an ordinary screen arrives. A host card burying a sheet would
/// strand it, so every arrival passes through here.
pub(crate) fn close_drawers(stack: &mut Stack) -> bool {
    // The guard for the powering-off frame belongs in the caller: refusing to pop here would leave
    // the sheet under whatever lands, so `land` refuses the card outright instead.
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
            if !s.is_overlay() {
                close_drawers(stack);
            }
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
        Transition::OverRoot(s) => {
            // Keep the Home root and the view it stands on. A sheet is not a view to come back
            // to, and it is the way in to one of these arrivals, so it goes first. The landing
            // cannot overflow: the pair and one screen fit any MAX_DEPTH.
            close_drawers(stack);
            stack.truncate(2);
            // On the idle Home there is no view, so the arrival itself is what the pair keeps. The
            // rider is already there: they land on the screen they left, with its cursor and any
            // message on it, rather than on a second fresh copy stacked over the first.
            if !stack.last().is_some_and(|top| top.row() == s.row()) {
                let _ = stack.push(s);
            }
        }
        Transition::Root(s) => {
            stack.truncate(1); // keep the Home root
            let r = stack.push(s);
            debug_assert!(r.is_ok(), "screen stack overflow — raise MAX_DEPTH");
        }
        Transition::Home => stack.truncate(1),
    }
}

/// Logic context handed to [`Screen::handle`]: the mutable app state a screen adjusts. The render
/// half is [`Render`].
pub struct Ctx<'a> {
    pub find: &'a mut crate::find_place::FindState,
    pub landmarks: &'a mut crate::landmarks::Landmarks,
    pub ahead: &'a mut crate::whats_next::AheadState,
    pub place_local: Option<(u8, u16)>,
    pub state: &'a mut AppState,
    pub activity: &'a mut Activity,
    /// The persisted device settings. The settings screens edit them in place; the change is
    /// detected by [`App::apply_gesture`](crate::App::apply_gesture) and flagged for the host to
    /// save.
    pub settings: &'a mut Settings,
    pub routes: &'a [RouteSummary],
    pub rides: &'a [RideEntry],
    pub trips: &'a [crate::trip::TripSummary],
    /// The device's trip progress records, at most one per trip key. A trip without one reads as
    /// not started.
    pub trip_progress: &'a [crate::trip::TripProgress],
    /// Where the active trip's next day meets the day before; `None` loads the day as it is.
    pub day_join: Option<crate::trip::DayJoin>,
    /// The App-owned POI-list snapshot, read-only: the POI list's `Gesture::Press` reads the
    /// highlighted [`Poi`](obc_reader::Poi) out of it to hand to the detail screen.
    pub poi_scratch: &'a PoiScratch,
    /// The App-owned route-corridor POI snapshot, read-only, so `handle` sees exactly the rows
    /// `draw` drew. Empty until a snapshot lands.
    pub corridor: &'a [obc_reader::CorridorPoi],
    /// The live BLE sensor scan hits, read-only. Empty outside a scan.
    pub sensor_scan_hits: &'a [crate::sensors::SensorScanHit],
    /// Whether the panel has a controllable light. The quick drawer's root row is one control
    /// shorter without one.
    pub backlight: bool,
    /// The Navigator domain: the planning screens name what they want to it (`admit_intent`)
    /// rather than latching a request of their own.
    pub navigator: &'a mut crate::navigator::NavigatorMachine,
    /// The Recorder domain: the ride screens name the rider's start, save or discard to it
    /// (`request`) as the gesture happens.
    pub recorder: &'a mut crate::recorder::RecorderMachine,
    /// The DFU domain: the Firmware and update-confirm screens post their phase here.
    pub dfu: &'a mut crate::dfu::DfuState,
    /// The StorageInfo domain: the System screen asks for a free-space refresh on entry.
    pub storage: &'a mut crate::device_core::storage_info::StorageInfo,

    pub now_ms: u32,
}

impl Ctx<'_> {
    /// The base facts a context row's availability and value read, from an input context. The same
    /// answer [`Render::context_facts`] gives the frame that drew the row.
    pub(crate) fn context_facts(&self) -> context_drawer::ContextFacts<'_> {
        context_drawer::ContextFacts {
            state: self.state,
            navigation: self.navigator.route_state(),
            settings: self.settings,
            recording: self.recorder.recording(),
        }
    }
}

#[cfg(test)]
pub(crate) fn test_ctx<'a>(state: &'a mut AppState, activity: &'a mut Activity, settings: &'a mut Settings) -> Ctx<'a> {
    // The screens under test read these but never fill them, so one immutable `'static` each
    // serves every caller. A test-local temporary could not outlive this call.
    static EMPTY_SCRATCH: PoiScratch = PoiScratch::new();
    Ctx {
        find: Box::leak(Box::new(crate::find_place::FindState::new())),
        landmarks: Box::leak(Box::new(crate::landmarks::Landmarks::new())),
        ahead: Box::leak(Box::new(crate::whats_next::AheadState::new())),
        place_local: None,
        state,
        activity,
        settings,
        routes: &[],
        rides: &[],
        trips: &[],
        trip_progress: &[],
        day_join: None,
        backlight: true,
        poi_scratch: &EMPTY_SCRATCH,
        corridor: &[],
        sensor_scan_hits: &[],
        navigator: Box::leak(Box::new(crate::navigator::NavigatorMachine::new())),
        recorder: Box::leak(Box::new(crate::recorder::RecorderMachine::new())),
        dfu: Box::leak(Box::new(crate::dfu::DfuState::new())),
        storage: Box::leak(Box::new(crate::device_core::storage_info::StorageInfo::new())),

        now_ms: 0,
    }
}

/// The currently-tracked climb: the active [`ClimbSeg`] with its resident detail [`ClimbProfile`],
/// both borrowed from Navigator's climb state. Present exactly when Navigator's `active_climb` is
/// `Some`, so the two are always consistent and a screen never draws a stale buffer.
#[derive(Clone, Copy)]
pub struct ActiveClimb<'a> {
    pub seg: &'a ClimbSeg,
    /// Scoped to this climb's `[start_m, end_m]`; refilled only on climb entry, so free to read per
    /// frame.
    pub profile: &'a ClimbProfile,
}

/// Render context handed to [`Screen::draw`]: the read-only state plus the map `Reader`, the host's
/// borrowed `RenderScratch`, and the in-flight Select hold-progress (0.0–1.0) the guarded-action
/// confirm ring fills with.
pub struct Render<'a> {
    pub visit_target: Option<obc_route::visit::VisitTarget>,
    pub find: &'a crate::find_place::FindState,
    pub landmarks: &'a crate::landmarks::Landmarks,
    pub(crate) map_icons: &'a crate::map_icons::MapIcons,
    pub(crate) settlements: &'a crate::settlements::SettlementCache,
    pub ahead: &'a crate::whats_next::AheadState,
    pub peak_view: Option<&'a crate::peak_view::Panorama>,
    /// The frame's borrowed render scratch: the host owns it and lends it for this call. It carries
    /// nothing between frames, so a screen that wants a presentation switch to stick states it per
    /// frame in an [`obc_render::RenderConfig`]. `None` for a chrome-only frame, which is exactly
    /// the set of frames that never reach the map scene's draw, the one place this is unwrapped.
    pub scratch: Option<&'a mut RenderScratch>,

    pub state: &'a AppState,
    /// The active route and live guidance facts, borrowed from Navigator without a copied mirror.
    pub navigation: &'a crate::navigator::RouteState,
    /// The Recorder domain, read-only: the ride's own numbers. Distance, moving time, climb, the
    /// live sensor values and the per-ride summary all come from the machine that owns the session
    /// they belong to; no screen keeps a copy of any of them.
    pub recorder: &'a crate::recorder::RecorderMachine,
    pub settings: &'a Settings,
    pub routes: &'a [RouteSummary],
    pub unaccepted_routes: u64,
    pub internal_routes: u64,
    pub rides: &'a [RideEntry],
    /// The names of the ride catalog's trips, one per trip key.
    pub ride_trips: &'a [crate::RideTrip],
    pub trips: &'a [crate::trip::TripSummary],
    /// The device's trip progress records, at most one per trip key. A trip without one reads as
    /// not started.
    pub trip_progress: &'a [crate::trip::TripProgress],
    /// Where the active trip's next day meets the day before; `None` loads the day as it is.
    pub day_join: Option<crate::trip::DayJoin>,
    /// The length of the loaded trip day's later days, or `None` when the loaded route is not a
    /// trip day.
    pub trip_later_m: Option<u32>,
    /// The active route's geometry (the Map strokes it), or `None` when no route is loaded.
    /// Host-owned, streamed on demand.
    pub route: Option<&'a RouteReader<'a>>,
    /// The active route's elevation profile, rebuilt on route load and cached; `None` when no route
    /// is loaded. Resident, so the screen never re-reads to draw.
    pub profile: Option<&'a Profile>,
    /// The viewed ride's recorded-track elevation profile, host-filled on detail entry and
    /// invalidated on exit. `None` while the fill still streams and on every other screen.
    pub ride_profile: Option<&'a Profile>,
    /// The climb the rider is currently on, or `None` between climbs. A `Some` means a climb is
    /// tracked and both halves are valid, so a screen never reads a stale detail buffer.
    pub climb: Option<ActiveClimb<'a>>,
    /// The active route's detected climbs, in route order. Empty when no route is loaded.
    pub climbs: &'a obc_route::Climbs,
    /// The active route's named-waypoint table, in route order. Empty when no route is loaded, so a
    /// screen iterates it unconditionally.
    pub waypoints: &'a Waypoints,
    /// The travelled-path breadcrumb; the Map strokes it under the route.
    pub breadcrumb: &'a Breadcrumb,
    /// Whether a ride is open. The riding views' chrome, the Firmware page's install row and the
    /// two delete rows all read it; none of them may keep a second copy.
    pub recording: bool,
    /// The previewed route's decimated shape polyline: at most 64 `(lon, lat)` µdeg points, keyed
    /// to the active route. Empty when it is missing or stale.
    pub nav_preview: &'a [(i32, i32)],
    /// The viewed ride's decimated recorded-track polyline, host-filled alongside the ride profile
    /// on detail entry and keyed to [`Activity::viewed_ride`](crate::Activity).
    pub ride_preview: &'a [(i32, i32)],
    /// The planned-but-uncommitted detour's decimated polyline, keyed to the active route. Empty
    /// when it is missing or stale.
    pub detour_preview: &'a [(i32, i32)],
    /// The App-owned POI-list snapshot buffer, read-only: the POI list draws the frozen snapshot
    /// its [`prepare`](Screen::prepare) pass already took, so draw is side-effect-free.
    pub poi_scratch: &'a PoiScratch,
    /// The frozen route-corridor POI snapshot the Up-ahead timeline merges with
    /// [`waypoints`](Render::waypoints), ascending by along-route distance.
    pub corridor: &'a [obc_reader::CorridorPoi],
    /// Whether that snapshot has settled: taken, or settled empty on a query error. `false` only
    /// while the query still waits for its inputs, which keeps the Up-ahead empty state from
    /// flashing an answer the next frame contradicts.
    pub corridor_status: obc_reader::reader::places::QueryProgress,
    /// The App-owned per-category "next ahead" cache behind the `Next: <category>` stat tiles,
    /// refreshed on a progress-keyed policy rather than per frame.
    pub next_ahead: &'a crate::next_ahead::NextAhead,
    /// The per-slot BLE sensor status. Empty defaults outside a connection, so the Sensors screen
    /// indexes it by slot unconditionally.
    pub sensor_status: &'a [crate::sensors::SensorStatus],
    /// The live BLE sensor scan hits. Empty outside a scan.
    pub sensor_scan_hits: &'a [crate::sensors::SensorScanHit],
    /// Panel size in device pixels. Every screen lays out in whole pixels; the Map computes its
    /// `f32` viewport locally.
    pub w: i32,
    pub h: i32,
    pub now_ms: u32,
    /// The frame's marquee: the one long name a draw asks to scroll instead of cutting. Written
    /// through a `Cell`, because the draw helpers borrow the frame shared; the render reads the
    /// request back after the draw.
    pub(crate) marquee: vocab::marquee::MarqueeFrame,
    /// The live wall-clock time this frame. For boot-relative millis a screen uses
    /// [`now_ms`](Render::now_ms) instead.
    pub now: DateTime,
    /// Whether [`now`](Render::now) has an established origin: a persisted, manual or GPS time has
    /// been applied. The Home date line draws only when set, so a date with no trusted origin is
    /// never shown; the `HH:MM` clock draws either way.
    pub clock_set: bool,
    pub place_local: Option<(u8, u16)>,
    pub hold_progress: f32,
    /// No current GPS fix this frame: none yet, or the last has gone stale. The riding views draw
    /// the "No GPS Fix" banner, and the Map suppresses the off-route pill because the match is
    /// stale.
    pub no_fix: bool,
    /// Microsecond clock for the map render's per-stage timing. Hosts that do not profile pass
    /// [`NoopClock`](obc_render::NoopClock); the device passes its `Instant`-based clock.
    pub clock: &'a dyn Clock,
    /// What the base screen's map render drew this frame, for the host's stats panel. Reset by the
    /// host each frame; only map-base screens write it.
    pub stats: RenderStats,
    /// The running firmware version string. Empty until the host feeds it.
    pub fw_version: &'a str,
    /// The loaded map's display name. Empty until map load.
    pub map_name: &'a str,
    /// The loaded map's OBCM format version; `0` means no map yet.
    pub map_obcm_version: u8,
    /// Free space on the medium in bytes, or `None` until a measurement answers the System screen's
    /// on-entry refresh.
    pub card_free_bytes: Option<u64>,

    /// Whether the panel has a controllable light. The quick drawer draws three icons instead of
    /// four without one.
    pub backlight: bool,
}

impl Render<'_> {
    /// The base facts a context row's availability and value read, from a draw context.
    pub(crate) fn context_facts(&self) -> context_drawer::ContextFacts<'_> {
        context_drawer::ContextFacts {
            state: self.state,
            navigation: self.navigation,
            settings: self.settings,
            recording: self.recording,
        }
    }

    /// What the Up-ahead timeline is scoped to this frame: the rider's live filter and their
    /// persisted source preference, read as one value.
    pub(crate) fn up_ahead_scope(&self) -> crate::corridor::UpAheadScope {
        crate::corridor::UpAheadScope { filter: self.state.up_ahead_filter, source: self.settings.up_ahead_source }
    }

    /// The narrow live-data view the stat-field catalogue formats from, and the one constructor of
    /// [`Readout`](crate::stat_fields::Readout), so `stat_fields` stays decoupled from the full
    /// draw context.
    pub fn readout(&self) -> crate::stat_fields::Readout<'_> {
        crate::stat_fields::Readout {
            fix: self.state.user_fix,
            navigation: self.navigation,
            recorder: self.recorder,
            units: self.settings.units,
            route: self.route,
            profile: self.profile,
            climb: self.climb,
            waypoints: self.waypoints,
            next_waypoint: self.navigation.next_waypoint,
            now: self.now,
            now_ms: self.now_ms,
            bike_type: self.settings.bike_type,
            language: self.settings.language,
            next_ahead: self.next_ahead,
            trip_later_m: self.trip_later_m,
        }
    }
}

/// One frame's draw context plus the base-map scene it streams. `None` on a chrome-only frame,
/// which is exactly the set of frames that never reach a map screen's draw.
///
/// Every chrome screen receives `&mut Render` through `Deref`, so the scene stays out of the
/// signatures of the screens that do not stream one.
pub struct RenderFrame<'a, 'd> {
    pub scene: Option<&'a Reader<'d>>,
    pub render: Render<'a>,
}

impl<'a> Deref for RenderFrame<'a, '_> {
    type Target = Render<'a>;

    fn deref(&self) -> &Self::Target {
        &self.render
    }
}

impl DerefMut for RenderFrame<'_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.render
    }
}

/// The narrow pre-draw acquisition context: everything a screen needs to resolve its reader-backed
/// one-shot state before drawing. Handed to [`Screen::prepare`], which runs on the base screen once
/// per frame ahead of the draw loop, so the side-effectful reads happen here and
/// [`draw`](Screen::draw) then consumes immutable prepared state.
pub struct Prepare<'a, 'd> {
    pub place_local: Option<(u8, u16)>,
    /// The streamed-map `Reader`, or `None` when the host did not build it this frame. The POI
    /// acquisitions retry next frame until
    /// [`base_needs_reader`](crate::App::base_needs_reader) stops asking the board to build it.
    pub reader: Option<&'a Reader<'d>>,
    /// The active streamed route geometry, if the host opened it this frame. The Skip-ahead chooser
    /// resolves its rejoin coordinate and selected-stretch bounds from it.
    pub route: Option<&'a RouteReader<'a>>,
    /// The App-owned POI-list snapshot buffer; the POI list fills it here.
    pub poi_scratch: &'a mut PoiScratch,
    /// The rider's current fix, `(lon, lat)` µdeg: the POI list's nearest-16 query origin.
    pub user_fix: Option<Fix>,
    /// Copy of the active route slot and live matched progress at this frame's prepare boundary.
    /// The Detour chooser advances its selection anchor with the rider before resolving geometry.
    pub active_route: Option<usize>,
    pub progress_m: u32,
    pub route_total_m: u32,
    /// The planned detour's decimated polyline, which the Detour preview folds into its fitted
    /// camera bounds. Empty for every other screen.
    pub detour_preview: &'a [(i32, i32)],
}

/// A screen's classification, declared in its `screens!` table row so it cannot drift from the
/// enum. Only the two behaviours that hang off it are kinds: what a screen *is* beyond them is
/// [`BaseContent`], which is the fact the map plane and the live-data policy read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenKind {
    /// Replaces the view below: every riding view, the Home root, the menus and the prompts.
    Base,
    /// Drawn *over* the screen below (the stack composites it on top).
    Overlay,
    /// Part of the settings subtree — edits are held un-persisted while one is on top.
    Settings,
}

impl ScreenKind {
    pub fn is_overlay(self) -> bool {
        matches!(self, ScreenKind::Overlay)
    }

    pub fn is_settings(self) -> bool {
        matches!(self, ScreenKind::Settings)
    }
}

/// What a screen's lowest-opaque draw is. The map-plane host gates whole pipelines off this one
/// declared fact rather than scattered `matches!` on the enum: building the streamed-map
/// [`Reader`], counting a screen as live-data, and showing the BLE connected indicator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseContent {
    /// Draws the streamed base map and therefore reads the [`Reader`] for the map itself.
    Map,
    /// A live riding view fed by the fix but not the map: a fresh fix redraws it, but it does no
    /// map I/O and shows no BLE indicator.
    LiveRiding,
    /// Static chrome. No live map or fix redraw; carries the BLE connected indicator in its title
    /// bar.
    Chrome,
}

/// Whether — and until when — a screen needs the streamed-map [`Reader`] built. Declared per screen
/// so the render-on-demand board host skips the per-frame `Reader` build (an SD style-table parse
/// and its stack spike) on every frame that does not need it. The POI variants take a one-shot read
/// at [`prepare`](Screen::prepare) time, so their need lasts only until that read resolves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReaderNeed {
    /// Selected article text, attribution, or Peak View availability.
    Articles,
    /// Never needs the `Reader` (all chrome and live-riding non-map screens).
    Never,
    /// Always needs it — any [`Map`](BaseContent::Map) base screen.
    Always,
    /// Needs it until the POI list's category snapshot has been taken.
    PoiSnapshot,
    /// Needs it until the POI detail's opening-hours read has resolved.
    PoiHours,
    /// Reads selected photo content during bounded preparation.
    Photo,
}

/// The exact facts a screen's draw reads, declared in its `screens!` table row so the repaint
/// decision is a property of the screen rather than of the call sites that mutate those facts.
///
/// The pass builds the visible stack's key before and after its stages and dirties the map when the
/// two differ. A screen whose content moves only on input or on its own
/// [`tick_timers`](Screen::tick_timers) declares [`Static`](RenderKeyKind::Static) and contributes
/// only its identity, which is enough for a navigation to repaint.
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

    /// The Up-ahead timeline: live route progress, the route's length, and the corridor snapshot
    /// the rows are merged from. The Assistant list shares it for its What's next hint.
    UpAhead,
    Drawer,
}

/// The capability metadata for one screen, declared in its `screens!` table row so cross-cutting UI
/// policy is a single declaration that cannot drift from the enum. A fact earns a field here by
/// having a reader: the timer, hold-fill and rescan fan-outs match on the variant, so they declare
/// nothing.
///
/// Built with the const archetype constructors ([`nav`](Caps::nav), [`map`](Caps::map),
/// [`riding`](Caps::riding), [`settings`](Caps::settings), [`modal`](Caps::modal)) and refined with
/// the const chaining setters, so a row reads `Caps::nav().exempt()`. Not stored in the [`Screen`]
/// enum, which would inflate every stack slot; it compiles to a `const` per variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    /// The [`ScreenKind`] the overlay and settings behaviours hang off.
    pub kind: ScreenKind,
    /// What the screen's base draw is. Gates map I/O, live-data redraw, and the BLE indicator.
    pub base: BaseContent,
    /// Idle-return exempt: a modal card or wait the idle-return timeout must never take away.
    pub idle_exempt: bool,
    /// A deliberate ride view: stays put on the idle timeout while a ride is tracked, instead of
    /// returning to the Map.
    pub ride_view: bool,
    /// A deliberate browse view when not tracking: the idle timeout treats it as intentional, so it
    /// is not returned to Home.
    pub browse_exempt: bool,
    /// Whether — and until when — the screen needs the streamed-map [`Reader`] at draw.
    pub reader: ReaderNeed,
    /// A genuinely blocking modal: while it is on top the device-wide drawer chords are refused, so
    /// a squeeze cannot open a drawer over a pairing passkey or a running map transfer. Every other
    /// screen is eligible, because the quick drawer is global by design.
    pub blocks_chords: bool,
    /// A decision the rider must answer: while it is on top the global Back-hold escape does not
    /// leave it. A superset of [`blocks_chords`](Caps::blocks_chords), so
    /// [`blocking`](Caps::blocking) sets both.
    pub blocks_escape: bool,

    /// Whether a drawer recesses this screen: the sheet lifts off a base drawn one device-64 level
    /// down ([`dim_color`]), so the eye reads the sheet as being in front of a page.
    ///
    /// The recess *is* a second draw of the base, which is why this decides more than a colour: a
    /// screen declaring `true` keeps being drawn under a sheet, and only one declaring `false` can
    /// have its draw skipped and its rows left standing. `false` for a map base, whose second draw
    /// is a whole map render, and for a prepared photo, which cannot decode in draw.
    pub recess: bool,
    /// The exact facts this screen's draw reads. The pass compares them before and after its stages
    /// and dirties the map when they move.
    pub render_key: RenderKeyKind,
}

impl Caps {
    /// Navigation chrome. The neutral base every other archetype refines from.
    pub const fn nav() -> Self {
        Caps {
            kind: ScreenKind::Base,
            base: BaseContent::Chrome,
            idle_exempt: false,
            ride_view: false,
            browse_exempt: false,
            reader: ReaderNeed::Never,
            blocks_chords: false,
            blocks_escape: false,

            recess: true,
            render_key: RenderKeyKind::Static,
        }
    }

    /// A drawer: an overlay sheet composited over the still-visible base. Whether that base is
    /// drawn again through the dim LUT is the base's own [`recess`](Caps::recess) declaration, not
    /// the sheet's. Its key kind shadows the base's, which is what freezes it.
    pub const fn overlay() -> Self {
        Caps { kind: ScreenKind::Overlay, render_key: RenderKeyKind::Drawer, ..Caps::nav() }
    }

    /// A map-base screen: reads the `Reader` every frame, is both a tracking ride view and a
    /// deliberate browse view when not tracking, and is not recessed under a drawer, because its
    /// second draw would be a whole map render.
    pub const fn map() -> Self {
        Caps {
            base: BaseContent::Map,
            ride_view: true,
            browse_exempt: true,
            reader: ReaderNeed::Always,
            render_key: RenderKeyKind::Map,
            recess: false,
            ..Caps::nav()
        }
    }

    /// A live riding view fed by the fix but not the map: redrawn on a fresh fix, no map I/O.
    pub const fn riding() -> Self {
        Caps { base: BaseContent::LiveRiding, ride_view: true, render_key: RenderKeyKind::Statistics, ..Caps::nav() }
    }

    /// A settings-subtree screen: a pending save is held un-persisted while one is on top.
    pub const fn settings() -> Self {
        Caps { kind: ScreenKind::Settings, ..Caps::nav() }
    }

    /// A modal card or wait: idle-return exempt until it is dismissed or answered.
    pub const fn modal() -> Self {
        Caps { idle_exempt: true, ..Caps::nav() }
    }

    pub const fn ride_view(mut self) -> Self {
        self.ride_view = true;
        self
    }

    pub const fn exempt(mut self) -> Self {
        self.idle_exempt = true;
        self
    }

    /// A screen that refuses the drawer chords refuses the global escape as well, so this sets
    /// both.
    pub const fn blocking(mut self) -> Self {
        self.blocks_chords = true;
        self.blocks_escape = true;
        self
    }

    pub const fn blocks_escape(mut self) -> Self {
        self.blocks_escape = true;
        self
    }

    pub const fn reader(mut self, need: ReaderNeed) -> Self {
        self.reader = need;
        self
    }

    pub const fn key(mut self, render_key: RenderKeyKind) -> Self {
        self.render_key = render_key;
        self
    }
}

/// The one screen table. Each row is `Variant(StateType) => Caps`; the macro expands it into the
/// [`Screen`] enum, the `handle`/`draw` delegation matches, and the per-screen capability metadata.
/// There is no second list of variants to keep in sync, and a cross-cutting policy is an explicit
/// capability on the row rather than a forgotten `matches!` elsewhere. A row and its module are not
/// the whole of a new screen: it also needs its strings in the four catalogs, a way in, and a sweep
/// frame with its digest row. Deliberately a dumb token-pasting table, not a framework.
macro_rules! screens {
    ($( $(#[$doc:meta])* $variant:ident($state:ty) => $caps:expr, )+) => {
        /// The on-device screens. Each variant owns its typed state and forwards to that screen's
        /// inherent `handle` and `draw`.
        pub enum Screen {
            $( $(#[$doc])* $variant($state), )+
        }

        /// A screen's identity, without its state: one byte per visible row in a
        /// [`RenderKey`](crate::render_key).
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(u8)]
        pub enum ScreenRow {
            $( $variant, )+
        }

        impl Screen {
            /// Handle one gesture, returning the navigation [`Transition`] it triggers.
            ///
            /// [`Gesture::BackHold`] never arrives here: it is the global escape, resolved in
            /// [`App::apply_gesture`](crate::App::apply_gesture) above this dispatch. Every
            /// screen's `BackHold` arm is inert by construction, and this assert says so loudly if
            /// a host ever routes one past the escape.
            pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
                debug_assert!(!matches!(g, Gesture::BackHold), "Back-hold is the global escape, not a screen gesture");
                match self {
                    $( Screen::$variant(s) => s.handle(g, cx), )+
                }
            }

            /// Draw the screen into the frame's [`Canvas`]. The two host generics stop here: every
            /// screen below draws through `&mut impl Surface`, except the Map, which reaches the
            /// raw target via [`Canvas::split`] for its `RenderScratch` calls.
            pub fn draw<D, F>(&self, cv: &mut Canvas<D, F>, rx: &mut RenderFrame<'_, '_>)
            where
                D: DrawTarget,
                F: Fn(u16) -> D::Color,
            {
                match self {
                    $( Screen::$variant(s) => s.draw(cv, rx), )+
                }
            }

            /// This screen's [`Caps`], exactly as declared in its `screens!` table row: the single
            /// authority for its cross-cutting UI policy. Every other classifier reads it instead
            /// of matching on the variant again.
            pub fn caps(&self) -> Caps {
                match self {
                    $( Screen::$variant(_) => $caps, )+
                }
            }

            pub fn kind(&self) -> ScreenKind {
                self.caps().kind
            }

            /// This screen's identity as a [`ScreenRow`], which is what a render key records for
            /// each visible row, so a navigation moves the key without any screen saying so.
            pub fn row(&self) -> ScreenRow {
                match self {
                    $( Screen::$variant(_) => ScreenRow::$variant, )+
                }
            }

            /// This screen's variant name, for example `"Map"`. The web demo host publishes it so
            /// the landing page advances a guided demo only once the app reached the target
            /// screen.
            pub fn name(&self) -> &'static str {
                match self {
                    $( Screen::$variant(_) => stringify!($variant), )+
                }
            }

            /// Every screen's variant name, in `screens!` table order. The web demo host exports
            /// this as the landing page's drift guard: a tour scripted against a screen name that
            /// does not exist fails CI instead of stalling.
            pub const NAMES: &'static [&'static str] = &[ $( stringify!($variant), )+ ];

            /// Every screen's declared [`Caps`], in `screens!` table order, paired index-for-index
            /// with [`NAMES`](Screen::NAMES). The invariant tests enumerate it without having to
            /// construct each variant's state.
            pub const CAPS: &'static [Caps] = &[ $( $caps, )+ ];
        }
    };
}

screens! {
    Home(HomeScreen) => Caps::nav().key(RenderKeyKind::Home),
    Map(MapScreen) => Caps::map(),
    Assistant(AssistantScreen) => Caps::nav().key(RenderKeyKind::UpAhead),
    Journey(JourneyScreen) => Caps::nav(),
    Landmarks(LandmarksScreen) => Caps::map(),
    PeakArticle(PeakArticleScreen) => Caps::nav().reader(ReaderNeed::Articles),
    LandmarkSources(LandmarkSourcesScreen) => Caps::nav().reader(ReaderNeed::Articles),
    LandmarkPhoto(LandmarkPhotoScreen) => Caps { recess: false, ..Caps::nav().ride_view().reader(ReaderNeed::Photo) },
    Statistics(StatisticsScreen) => Caps::riding(),
    /// The current climb's grade-striped elevation profile, cursor, and four climb-scoped tiles.
    Climb(ClimbScreen) => Caps::riding().key(RenderKeyKind::Climb),
    /// The pause page: ride-so-far ledger and the guarded Resume / Finish / Discard rows.
    RideControl(RideControl) => Caps::nav().ride_view(),
    /// The route-less start card: *Start ride* begins a tracking session with no route.
    RideStart(RideStartScreen) => Caps::nav(),
    /// The one-shot boot decision for a durable recording recovered after reset. Back cannot
    /// dismiss it; Continue preserves restored totals, while Discard is hold-guarded.
    RideRecovery(RideRecoveryScreen) => Caps::modal().blocks_escape(),
    Menu(MenuScreen) => Caps::nav(),
    /// Heading-relative three-depth terrain panorama with named summit selection. Its profile is
    /// platform-fed, so the screen is unreachable when no panorama data is installed.
    PeakView(PeakViewScreen) => Caps::riding().reader(ReaderNeed::Articles),
    /// The detour chooser: a map base with streamed skipped-stretch ink and an auto-fit camera.
    Detour(DetourScreen) => Caps::map(),
    /// The planned detour and its cost line over the map; Press commits the splice.
    DetourPreview(DetourPreviewScreen) => Caps::map(),
    /// The "Up ahead" timeline: the route-ordered merge of the waypoint table and the corridor-POI
    /// snapshot. It holds neither the rows nor the scope it reads them under — the category filter
    /// and the source scope are rows of the [context sheet](context_drawer::UP_AHEAD) above it.
    WhatsNext(WhatsNextScreen) => Caps::nav().key(RenderKeyKind::UpAhead),
    FindPlace(FindPlaceScreen) => Caps::map(),
    VisitReview(VisitReviewScreen) => Caps::map(),
    /// One category's distance-sorted nearest-16 with live bearing arrows.
    PoiList(PoiListScreen) => Caps::nav().reader(ReaderNeed::PoiSnapshot),
    /// The map-first comparison of one frozen, measured alternative candidate.
    Easier(EasierScreen) => Caps::map(),
    /// A single place's detail: full name, subtype, live bearing arrow, today's hours.
    PoiDetail(PoiDetailScreen) => Caps::nav().reader(ReaderNeed::PoiHours),
    /// The route-planning wait: a spinning needle while the host steps the resumable router. Back
    /// cancels; the host's answer replaces it with the computed-route overview or the failure card.
    NavPlanning(NavPlanningScreen) => Caps::modal(),
    /// The route-planning failure card. Info-only: any press or Back returns to the detail.
    NavFail(NavFailScreen) => Caps::nav(),
    RouteMenu(RouteMenuScreen) => Caps::nav(),
    /// The trip cascade-delete confirm, reached by long-pressing a trip folder row. A completed
    /// hold records the trip's durable id for the host to delete the trip and its member routes.
    RouteCleanup(RouteCleanupScreen) => Caps::nav(),
    TripDelete(TripDeleteScreen) => Caps::nav(),
    /// The stored-rides list, with a trip's rides in one folder; a folder press pushes the trip's
    /// rides, a ride press opens the Ride detail.
    Rides(RidesScreen) => Caps::nav(),
    /// The recorded twin of the Route overview: the ridden track on the device map, then the
    /// profile, each over a stat ledger, and the guarded Delete-ride row. A static map base.
    RideDetail(RideDetailScreen) => Caps { base: BaseContent::Map, reader: ReaderNeed::Always, recess: false, ..Caps::nav() },
    /// The route on the device map over its stats, then the full-height profile. A map base
    /// for the map band, but a static page: it is no ride view and redraws on no fix.
    RouteOverview(RouteOverviewScreen) => Caps { base: BaseContent::Map, reader: ReaderNeed::Always, recess: false, ..Caps::nav() },
    /// START RIDE away from the route start: Ride to start, Join nearest, or Cancel.
    StartAway(StartAwayScreen) => Caps::nav(),
    /// The end of the loaded route during a ride: Finish ride, Ride on to the next trip day, or
    /// Keep riding. Only the card scheduler opens it, and it waits until the rider has closed
    /// everything over the riding page.
    Arrival(ArrivalScreen) => Caps::modal(),
    RouteSwap(RouteSwapScreen) => Caps::nav().exempt(),
    /// The idle route-upload prompt: Start navigation or Dismiss. Host-pushed, and auto-closes
    /// after [`UPLOAD_POPUP_TIMEOUT_MS`]. Advisory: the route is already committed.
    RouteReceived(RouteReceivedScreen) => Caps::modal(),
    /// The active-route-replaced info card. Adoption already happened when it opens, so this only
    /// tells the rider. Dismissed by any press or Back, or by the same auto-close.
    RouteUpdated(RouteUpdatedScreen) => Caps::modal(),
    /// The trip-received popup. A committed trip upload always lands after its member routes, so
    /// it replaces the last per-route popup of the burst. It holds the trip's durable id, not a
    /// catalog index, so no rescan remap is needed.
    TripReceived(TripReceivedScreen) => Caps::modal(),
    /// The BLE pairing passkey card. Host-pushed when the seam's passkey goes `Some`, popped when
    /// it clears. Opaque and non-dismissible.
    Passkey(PasskeyScreen) => Caps::modal().blocking(),
    /// The map-transfer card: the one screen a multi-minute SD write is visible on. Host-pushed
    /// when a map upload starts. Non-dismissible while bytes land, dismissable once terminal.
    MapTransfer(MapTransferScreen) => Caps::modal().blocking(),
    /// The advisory warning card: missing sensors, or a slow (fragmented) map. Host-pushed,
    /// coalesced, and dismissed on any press.
    Warning(WarningScreen) => Caps::modal(),
    /// The Settings hub and its pages: one screen type over a row table each. The value rows open
    /// the drawer editor as a sheet over the page.
    Settings(SettingsPage) => Caps::settings(),
    Ride(SettingsPage) => Caps::settings(),
    Display(SettingsPage) => Caps::settings(),
    /// The Connections page: the Bluetooth switch, the phone's status and Forget, and the door to
    /// the sensors. Its status lines follow the sensor slots, so it keys on them.
    Connections(SettingsPage) => Caps::settings().key(RenderKeyKind::SensorSettings),
    Power(SettingsPage) => Caps::settings(),
    System(SettingsPage) => Caps::settings(),
    DateTime(SettingsPage) => Caps::settings(),
    Firmware(SettingsPage) => Caps::settings(),
    StatFields(StatFieldsScreen) => Caps::settings(),
    AddField(AddFieldScreen) => Caps::settings(),
    /// The BLE-sensor pages: their rows draw the per-slot status, so they key on it.
    Sensors(SensorsScreen) => Caps::settings().key(RenderKeyKind::SensorSettings),
    SensorScan(SensorScanScreen) => Caps::settings().key(RenderKeyKind::SensorSettings),
    /// The Language pick list.
    Language(LanguageScreen) => Caps::settings(),
    About(AboutScreen) => Caps::settings(),
    Reset(ResetScreen) => Caps::settings(),
    /// The "Checking update..." wait while the board validates the staged package. The answer
    /// replaces it with the confirm screen or an error card.
    DfuCheck(DfuCheckScreen) => Caps::modal(),
    /// The install confirm: installed and update versions, the no-undo and same-version warnings,
    /// and the Install / Cancel rows.
    DfuConfirm(DfuConfirmScreen) => Caps::modal(),
    /// The "Preparing update..." spinner while the install one-shot waits for the board's drain.
    DfuProgress(DfuProgressScreen) => Caps::modal(),
    /// The terminal "Installing update" card, pushed right before the warm reset. It is the last
    /// painted frame, which the MIP panel holds through the whole install.
    DfuInstalling(DfuInstallingScreen) => Caps::modal().blocking(),
    /// The scan-error card: a typed [`DfuScanError`](crate::dfu::DfuScanError) as a plain sentence.
    DfuError(DfuErrorScreen) => Caps::modal(),
    /// The one-time "Updated to vX" toast, pushed on the first healthy boot after an update.
    DfuUpdated(DfuUpdatedScreen) => Caps::modal(),
    /// The one-time "UPDATE FAILED" card, pushed on the first boot after an armed update that did
    /// not end with the staged image running.
    DfuFailed(DfuFailedScreen) => Caps::modal(),
    /// The universal quick drawer: the top sheet Up+Select opens from anywhere the chord is not
    /// suppressed. Four unlabelled device-wide controls — brightness, the BLE radio, central
    /// settings, power — plus the nested brightness editor and the guarded power confirmation.
    QuickDrawer(QuickDrawerScreen) => Caps::overlay(),
    /// The contextual drawer: the bottom sheet Down+Back opens on a screen that declares a
    /// [`ContextMenu`]. It holds no content of its own — the rows come from the base screen's
    /// [`context`](Screen::context) declaration — so one row here serves every context.
    ContextDrawer(ContextDrawerScreen) => Caps::overlay(),
}

impl Screen {
    /// Whether this screen draws over the one below rather than replacing the view.
    pub fn is_overlay(&self) -> bool {
        self.kind().is_overlay()
    }

    /// Whether this overlay still owes the screen below a draw.
    ///
    /// The frozen base is not redrawn while a sheet purely covers it. Three things stop a sheet
    /// doing only that: a page slide, whose pages travel through the inset margin and whose
    /// differing heights shrink the sheet; a shorter sheet swapped in for a taller one; and one
    /// drawer replacing the other, whose rows at the opposite edge are still on the panel. One draw
    /// answers all three, because covering is cheap and uncovering is not. The answer is a debt
    /// carried until a frame pays it — see [`clear_base_debt`](Screen::clear_base_debt).
    pub(crate) fn needs_base(&self) -> bool {
        match self {
            Screen::QuickDrawer(s) => s.motion.needs_base(),
            Screen::ContextDrawer(s) => s.motion.needs_base() || s.draws_hero(),
            _ => false,
        }
    }

    /// Discharge that debt, called on every overlay by the frame that actually drew the base. A
    /// pass may tick and then draw no frame at all, so nothing short of the draw itself may end
    /// it.
    pub(crate) fn clear_base_debt(&mut self) {
        match self {
            Screen::QuickDrawer(s) => s.motion.clear_base_debt(),
            Screen::ContextDrawer(s) => s.motion.clear_base_debt(),
            _ => {}
        }
    }

    /// Arm that debt on the drawer that replaces the other one: the departed sheet's rows are
    /// still on the panel, and a sheet arriving from the opposite edge does not cover them.
    pub(crate) fn owe_base_draw(&mut self) {
        match self {
            Screen::QuickDrawer(s) => s.motion.owe_base(),
            Screen::ContextDrawer(s) => s.motion.owe_base(),
            _ => {}
        }
    }

    /// The contextual content this screen declares: the rows the Down+Back sheet offers over it,
    /// or `None` when it has no secondary actions and the chord therefore does nothing. Data, never
    /// behaviour — [`ContextDrawerScreen`] owns the cursor, the dimming, the transitions and the
    /// drawing, so a screen joins the grammar by naming a table. Intentionally partial: most
    /// screens declare nothing, and an empty sheet is worse than no sheet.
    pub(crate) fn context(&self) -> Option<&'static ContextMenu> {
        match self {
            // The Map adds the one row only it has a referent for: a scale-bar switch means nothing
            // on Statistics or the paused page.
            Screen::Map(_) => Some(&context_drawer::MAP),
            // The ride's secondary actions do not change because the rider switched readout.
            Screen::Statistics(_) | Screen::Climb(_) | Screen::RideControl(_) => Some(&context_drawer::RIDE),
            Screen::WhatsNext(_) => Some(&context_drawer::UP_AHEAD),
            Screen::FindPlace(_) | Screen::PoiList(_) => Some(&context_drawer::FIND_PLACE),
            Screen::Landmarks(_) | Screen::PeakArticle(_) => Some(&context_drawer::LANDMARK_CONTENT),
            Screen::LandmarkPhoto(photo) if photo.linked => Some(&context_drawer::LANDMARK_CONTENT),

            // The one screen whose next press consumes the routing profile. Not `NavPlanning`,
            // which already captured it, and not `RouteOverview`, whose BIKE TYPE row promises the
            // profile the route was planned under.
            Screen::PoiDetail(_) => Some(&context_drawer::ROUTE_PLAN),
            _ => None,
        }
    }

    /// Run the one-shot preparation for the reader-backed screens that need it before drawing.
    /// Intentionally partial: the other screens have no preparation.
    pub(crate) fn prepare(&mut self, px: &mut Prepare) {
        match self {
            Screen::PoiList(s) => s.prepare(px),
            Screen::PoiDetail(s) => s.prepare(px),
            Screen::Detour(s) => s.prepare(px),
            Screen::DetourPreview(s) => s.prepare(px),
            Screen::StartAway(s) => s.prepare(px),
            _ => {}
        }
    }

    /// The rectangle this screen's hold fill draws in, for a region-only repaint of a hold step.
    /// `None` on a screen that has not declared one, which a host then repaints in full.
    pub(crate) fn hold_fill_region(&self, w: i32, h: i32) -> Option<Rectangle> {
        match self {
            Screen::RouteOverview(_) => Some(RouteOverviewScreen::hold_fill_region(w, h)),
            Screen::RideDetail(_) => Some(RideDetailScreen::hold_fill_region(w, h)),
            _ => None,
        }
    }

    /// Whether this screen's `draw` would fill a live hold bar for its current selection. A
    /// render-on-demand host repaints a charging hold only when the fill would actually draw.
    /// Intentionally partial: most screens draw nothing hold-driven.
    pub(crate) fn wants_hold_fill(
        &self,
        settings: &Settings,
        state: &crate::AppState,
        navigation: &crate::navigator::RouteState,
        recording: bool,
        routes: &[RouteSummary],
        rides: &[RideEntry],
    ) -> bool {
        match self {
            Screen::RideControl(s) => s.selection_is_guarded(),
            Screen::RideRecovery(s) => s.selection_is_guarded(),
            Screen::RouteSwap(s) => s.selection_is_guarded(),
            Screen::Arrival(s) => s.selection_is_guarded(),
            Screen::Reset(s) => s.hold_fill_active(),
            Screen::StatFields(s) => s.selection_is_deletable(settings),
            Screen::Connections(s) => s.selection_is_guarded(state),
            Screen::QuickDrawer(s) => s.selection_is_guarded(),
            Screen::Sensors(s) => s.selection_is_guarded(settings),
            Screen::RouteOverview(s) => s.selection_is_guarded(navigation, recording, routes),
            Screen::RideDetail(s) => s.selection_is_guarded(recording, rides.len()),
            Screen::RouteCleanup(s) => s.selection_is_guarded(),
            Screen::TripDelete(s) => s.selection_is_guarded(),
            _ => false,
        }
    }

    /// Poll this screen's time-driven content one frame: fire any timed change that is due and
    /// report the residual deadline to the next one, both computed from the same gating locals so
    /// "did it change" and "when next" cannot drift apart. [`ScreenTick::changed`] is how a
    /// render-on-demand host marks the map dirty; [`ScreenTick::next_wake_ms`] is what an
    /// event-driven host folds across the visible stack into a single wake deadline, so the M33
    /// sleeps rather than free-running the loop.
    ///
    /// Most screens change only on input or a fresh fix and return [`ScreenTick::idle`]. `w`/`h`
    /// are the last rendered frame's panel size, for the screens that report a dirty
    /// [`region`](ScreenTick::region); `0` before the first frame makes them abstain.
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
            Screen::QuickDrawer(s) => s.tick_timers(now_ms),
            Screen::ContextDrawer(s) => s.tick_timers(now_ms),
            Screen::Statistics(s) => s.tick_timers(now_ms, settings),
            Screen::Home(s) => s.tick_timers(now, ms_to_next_minute),
            Screen::Map(s) => {
                s.tick_timers(now_ms, now, ms_to_next_minute, w, pan_active, settings.map_clock, tracking)
            }
            Screen::Menu(s) => s.tick_timers(now_ms),
            // The upload popups' auto-close deadline only arms the host's wake; the removal itself
            // runs in the App's popup sweep.
            Screen::RouteReceived(s) => s.tick_timers(now_ms),
            Screen::RouteUpdated(s) => s.tick_timers(now_ms),
            Screen::TripReceived(s) => s.tick_timers(now_ms),
            Screen::RouteSwap(s) => s.tick_timers(now_ms),
            Screen::RouteOverview(s) => s.tick_timers(now_ms),
            Screen::RideDetail(s) => s.tick_timers(now_ms),
            Screen::NavPlanning(s) => s.tick_timers(now_ms, w, h),
            Screen::PeakView(s) => s.tick_timers(now_ms, w, h),
            Screen::DfuCheck(s) => s.tick_timers(now_ms, w, h),
            Screen::DfuProgress(s) => s.tick_timers(now_ms, w, h),

            _ => ScreenTick::idle(),
        }
    }
}

/// The result of one [`Screen::tick_timers`] poll: whether a timed change fired and how long until
/// the next one is due. Produced in one body per screen, so the two halves of the timing contract
/// share their gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenTick {
    /// A timed change fired this poll: the drawn output differs, so the map plane needs a repaint.
    pub changed: bool,
    /// Milliseconds until the next timed change is due, or `None` when no timer is pending.
    /// Strictly positive: a due timer fired this poll instead.
    pub next_wake_ms: Option<u32>,
    /// Where this poll's change is contained, in panel pixels. `None` means anywhere, the
    /// full-frame repaint every screen implies by default. `Some(r)` is the screen's promise that
    /// the drawn output differs from the previous frame only inside `r`, so a host may clip the
    /// repaint to it. Read only when [`changed`](ScreenTick::changed) fired.
    pub region: Option<Rectangle>,
}

impl ScreenTick {
    /// No timed content: nothing changed, nothing pending.
    pub const fn idle() -> Self {
        ScreenTick { changed: false, next_wake_ms: None, region: None }
    }
}

/// How long a route-upload popup stays up before it auto-closes. The timeout is a dismiss: the
/// popups are advisory, because the route is committed before any prompt, so expiring loses
/// nothing.
pub const UPLOAD_POPUP_TIMEOUT_MS: u32 = 30_000;

/// Start riding catalog route `i` from a non-tracking state: the camera seeded on the route's
/// start, [`Mode::Riding`], `active_route` pointed at it, a fresh tracking session, and a clean
/// `[Home, Map]` stack. Shared by the Route overview's START RIDE press and the route-received
/// popup's *Start navigation*, so the two cannot drift. An out-of-range `i` pops instead.
pub(crate) fn start_ride(cx: &mut Ctx, i: usize) -> Transition {
    let Some(route) = cx.routes.get(i) else {
        return Transition::Pop;
    };
    let (lon, lat) = (route.start_lon, route.start_lat);
    cx.navigator.load_route(i);
    begin_riding_session(cx.state, cx.activity, cx.recorder, lon, lat)
}

/// Start a route-less tracking session from a non-tracking state. Identical to [`start_ride`]
/// minus the route: no `active_route`, and the camera seeded on the rider's last fix rather than a
/// route start. The recorded ride saves and behaves exactly like a route-guided one, but there is
/// no route to navigate against, so the route-relative stats read `--`.
pub(crate) fn start_ride_routeless(cx: &mut Ctx) -> Transition {
    let (lon, lat) = cx.state.user_fix.map_or((cx.state.cam_lon, cx.state.cam_lat), |f| (f.lon, f.lat));
    cx.navigator.set_active_route(None);
    begin_riding_session(cx.state, cx.activity, cx.recorder, lon, lat)
}

/// The session-begin shared by [`start_ride`], [`start_ride_routeless`] and the landing of Ride to
/// start. The caller sets `active_route` first — the one thing that differs between the starts.
pub(crate) fn begin_riding_session(
    state: &mut AppState,
    activity: &mut Activity,
    recorder: &mut crate::RecorderMachine,
    lon: i32,
    lat: i32,
) -> Transition {
    state.enter_riding_view(lon, lat);
    activity.mode = Mode::Riding;
    recorder.request(crate::RecorderIntent::Start);
    Transition::Root(Screen::Map(MapScreen::new()))
}

/// The one gesture the riding views bind identically: `press` pauses tracking and opens the
/// Ride-control page. Each riding screen calls this from its `Press` arm.
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
/// Tuned to the 64-colour (RGB222) gamut: the panel has 4 levels per channel (0/85/170/255), so
/// each value is chosen for the quantized result. The trailing comment on each is the device-64 RGB
/// it lands on; `tests/palette.rs` asserts every one, so a retune that forgets to update a comment
/// fails the build.
pub mod palette {
    /// Pack 8-bit RGB into RGB565.
    pub const fn rgb565(r: u8, g: u8, b: u8) -> u16 {
        (((r as u16) >> 3) << 11) | (((g as u16) >> 2) << 5) | ((b as u16) >> 3)
    }

    // Device-64 has no warm off-white: any blue below 192 tints it yellow, so this is a clean
    // near-white and the wood, ink and amber carry the warmth instead.
    pub const PARCHMENT: u16 = rgb565(245, 243, 238); // → (255,255,255) white
    pub const PARCHMENT_SHADE: u16 = rgb565(180, 170, 105); // → (170,170,85) tan
    pub const HUD: u16 = rgb565(46, 37, 26); // → (0,0,0) near-black frame
    pub const WOOD: u16 = rgb565(150, 100, 40); // → (170,85,0) wood brown
    /// Lighter wood for inset borders and frame lines.
    pub const WOOD_LIGHT: u16 = rgb565(180, 168, 100); // → (170,170,85) tan
    pub const INK: u16 = rgb565(44, 33, 20); // → (0,0,0) text black
    /// Muted ink for secondary and sub-label text.
    pub const SUBTEXT: u16 = rgb565(110, 90, 58); // → (85,85,0) olive
    /// Hairline rule between list rows.
    pub const RULE: u16 = rgb565(180, 170, 100); // → (170,170,85) tan
    pub const AMBER: u16 = rgb565(227, 165, 43); // → (255,170,0) accent
    pub const WARNING: u16 = rgb565(192, 73, 46); // → (255,85,0) warning
    /// Faint neutral grey: the Home screensaver's contour lines and empty battery cells. Dim
    /// enough to sit behind the clock, bright enough to read as fine topographic lines.
    pub const CONTOUR: u16 = rgb565(96, 96, 96); // → (85,85,85) grey
    /// Green: the "on" state of a settings toggle pill, and the shallowest band of the Climb
    /// screen's grade ramp. The only green on the panel.
    pub const ON: u16 = rgb565(0, 170, 0); // → (0,170,0) green
    /// Yellow: the Climb screen's `3–6 %` grade band. Device-64 has a pure `(255,255,0)`, so it
    /// reads distinctly from amber.
    pub const YELLOW: u16 = rgb565(255, 255, 0); // → (255,255,0) yellow
    /// Red: the Climb screen's steepest grade band. Hotter than the [`WARNING`] orange, so the two
    /// never blur into one another on the stripes.
    pub const RED: u16 = rgb565(255, 0, 0); // → (255,0,0) red
    /// Apricot: the Climb screen's tile background. Warmer and lighter than Statistics' tan
    /// [`PARCHMENT_SHADE`], so the two riding views' grids read apart at a glance.
    pub const CLIMB_TILE: u16 = rgb565(255, 170, 85); // → (255,170,85) apricot
    /// Magenta: the planned route line on the Map. It lands on no base-map feature, so it always
    /// reads as "the line to follow".
    pub const ROUTE: u16 = rgb565(255, 0, 255); // → (255,0,255) magenta
    /// Blue: the planned detour's polyline, which must read apart from the magenta route it will
    /// replace, the warning-orange skipped span, and the navy breadcrumb behind it.
    pub const DETOUR: u16 = rgb565(0, 90, 255); // → (0,85,255) blue
    /// Navy: the recorded breadcrumb, stroked over the route and under the marker. Recessive, so
    /// the trail behind reads quieter than the magenta route ahead.
    pub const BREADCRUMB: u16 = rgb565(0, 0, 170); // → (0,0,170) navy
    /// Dark green: the start dot of a previewed track.
    pub const TRACK_START: u16 = rgb565(0, 90, 0); // → (0,85,0) dark green
    /// Dark red: the end dot of a previewed route. It stays apart from the magenta line it ends.
    pub const TRACK_END: u16 = rgb565(170, 0, 0); // → (170,0,0) dark red
    /// Trail red: a ridden track on the ride detail's map, apart from the magenta planned route.
    pub const TRAIL: u16 = rgb565(170, 0, 0); // → (170,0,0) dark red
}

/// One RGB222 channel level, stepped down. Index by the channel's stored level (0-3); the result is
/// another level, so the dimmed colour stays exactly on the device gamut and nothing has to be
/// re-quantized.
const DIM_LEVEL: [u8; 4] = [0, 1, 1, 2];

/// The dim policy a frame draws its base through while a drawer covers it: an RGB565 colour in, the
/// same colour one device-64 level darker out.
///
/// A colour function, not a layer. [`App::draw_frame`](crate::App) composes it with the host's own
/// `color_fn` for the base screen and hands the sheet the untouched one, so it costs no RAM: no
/// capture buffer, no second framebuffer, no alpha. `Canvas` resolves `color_fn` once per
/// primitive, so this runs per primitive and not per pixel.
pub(crate) fn dim_color(rgb565: u16) -> u16 {
    let r = DIM_LEVEL[((rgb565 >> 14) & 0x3) as usize];
    let g = DIM_LEVEL[((rgb565 >> 9) & 0x3) as usize];
    let b = DIM_LEVEL[((rgb565 >> 3) & 0x3) as usize];
    palette::rgb565(r * 85, g * 85, b * 85)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn caps_table_pairs_with_names() {
        assert_eq!(Screen::CAPS.len(), Screen::NAMES.len(), "one Caps per screen name");
        assert!(!Screen::CAPS.is_empty());
    }

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

    /// A screen that declares one capability must declare the companions it implies, so a
    /// mis-declared row fails here instead of silently mis-routing a policy at runtime.
    #[test]
    fn every_screen_capability_combination_is_valid() {
        for (name, c) in Screen::NAMES.iter().zip(Screen::CAPS) {
            match c.reader {
                ReaderNeed::Always => assert_eq!(c.base, BaseContent::Map, "{name}: Always-reader ⟺ Map base"),
                ReaderNeed::Never => assert_ne!(c.base, BaseContent::Map, "{name}: a Map base must read Always"),
                ReaderNeed::Articles => assert_ne!(c.base, BaseContent::Map, "{name}: article reads do not draw a map"),
                ReaderNeed::PoiSnapshot | ReaderNeed::PoiHours | ReaderNeed::Photo => {
                    assert_eq!(c.base, BaseContent::Chrome, "{name}: a POI reader screen is chrome-based");
                }
            }
            if c.base != BaseContent::Chrome {
                assert!(!c.idle_exempt, "{name}: a map or riding base is not a modal exemption");
            }
            // A static page may draw a map, so the base content alone does not make a live view.
            if c.base != BaseContent::Chrome && c.render_key != RenderKeyKind::Static {
                assert!(c.ride_view, "{name}: a live-data base must be a ride view");
            }

            if c.browse_exempt {
                assert_eq!(c.base, BaseContent::Map, "{name}: only a Map base is browse-exempt");
            }
            if c.idle_exempt {
                assert_eq!(c.base, BaseContent::Chrome, "{name}: an idle-exempt modal is chrome-based");
                assert!(!c.ride_view, "{name}: an idle-exempt modal is not a ride view");
            }
            if c.kind == ScreenKind::Settings {
                assert_eq!(c.base, BaseContent::Chrome, "{name}: a settings screen is chrome-based");
                assert_eq!(c.reader, ReaderNeed::Never, "{name}: a settings screen needs no reader");
                assert!(!c.ride_view && !c.idle_exempt && !c.browse_exempt, "{name}: settings carry no view policy");
            }
            // Whether a drawer recesses a screen follows from its render cost, which the base
            // content already states, so a new map-class screen cannot arrive dimmed by accident.
            assert_eq!(
                c.recess,
                c.base != BaseContent::Map && c.reader != ReaderNeed::Photo,
                "{name}: streamed maps and prepared photos retain their pixels under a sheet"
            );
            // Declaring the Overlay kind without the Drawer key, or the other way round, would
            // either dim a base that keeps repainting or freeze one that never recedes.
            assert_eq!(
                c.kind.is_overlay(),
                c.render_key == RenderKeyKind::Drawer,
                "{name}: the Drawer key kind and the Overlay screen kind are one declaration"
            );
            if c.kind.is_overlay() {
                assert_eq!(c.base, BaseContent::Chrome, "{name}: a sheet draws chrome over the base it covers");
                assert!(!c.idle_exempt, "{name}: a drawer is not a modal the idle return must respect");
                assert!(!c.blocks_chords, "{name}: a drawer must not suppress the chord that closes it");
            }
            if c.blocks_chords {
                assert!(c.idle_exempt, "{name}: only an idle-exempt modal is blocking enough to refuse a chord");
                assert!(c.blocks_escape, "{name}: a screen that refuses a chord must refuse the escape too");
            }
            if c.blocks_escape {
                assert!(c.idle_exempt, "{name}: only a modal the rider must answer may refuse the escape");
            }
        }
    }

    /// The screens a drawer does not dim, by name. The invariant above ties the property to the
    /// base content; this list ties it to the rider's experience, so a change to either shows up in
    /// review as a change to both.
    #[test]
    fn streamed_map_and_photo_screens_are_left_undimmed_under_a_sheet() {
        let undimmed: std::vec::Vec<&str> =
            Screen::NAMES.iter().zip(Screen::CAPS).filter(|(_, c)| !c.recess).map(|(n, _)| *n).collect();
        assert_eq!(
            undimmed,
            [
                "Map",
                "Landmarks",
                "LandmarkPhoto",
                "Detour",
                "DetourPreview",
                "FindPlace",
                "VisitReview",
                "Easier",
                "RideDetail",
                "RouteOverview"
            ],
            "streamed map and prepared photo pixels stay unchanged while covered"
        );
    }

    /// Pin the host stack-slot size. The board resource guard measures the ARM layout.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn screen_enum_size_is_bounded() {
        assert_eq!(core::mem::size_of::<Screen>(), 128, "review screen-stack growth explicitly");
    }
}
