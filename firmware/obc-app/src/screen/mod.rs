//! The screen system — `no_std`, zero-alloc, no retained widget tree. Screens are a
//! [`Screen`] enum dispatched by `match` (static dispatch), each variant a small module
//! with typed state. Navigation is a return value: [`handle`](Screen::handle) returns a
//! [`Transition`] that [`apply`] runs against a [`heapless::Vec`] stack.
//!
//! The shared context is split by role: [`Ctx`] is the logic half handed to `handle`
//! (mutable camera/mode + clock), [`Render`] is the draw half (read-only state plus the
//! `Reader`, the reusable `MapRenderer`, and the in-flight hold-progress for the confirm ring).

use core::fmt::Write;

use embedded_graphics::{draw_target::DrawTarget, prelude::Point, primitives::Rectangle};
use obc_reader::Reader;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, Clock, MapRenderer, RenderStats, Surface,
};
use obc_route::{ClimbProfile, ClimbSeg, Profile, RouteReader};

use crate::activity::{Activity, Mode};
use crate::app::AppState;
use crate::breadcrumb::Breadcrumb;
use crate::input::Gesture;
use crate::ride::RideSummary;
use crate::route::RouteSummary;
use crate::settings::{DateTime, Settings, Units};

mod climb;
mod home;
mod list;
mod map;
mod menu;
mod nav_route;
mod passkey;
mod poi_detail;
mod poi_list;
mod poi_menu;
mod ride_control;
mod ride_start;
mod rides;
mod route_menu;
mod route_overview;
mod route_received;
mod route_swap;
mod settings;
mod statistics;
mod warning;

pub use climb::ClimbScreen;
pub use home::HomeScreen;
pub use list::window_start;
pub use map::{MapScreen, ROUTE_WEIGHT};
pub use menu::MenuScreen;
pub use nav_route::{needle_region, NavConfirmScreen, NavFailScreen, NavPlanningScreen};
pub use passkey::PasskeyScreen;
pub use poi_detail::PoiDetailScreen;
pub use poi_list::{PoiListScreen, PoiScratch};
pub use poi_menu::PoiMenuScreen;
pub use ride_control::RideControl;
pub use ride_start::RideStartScreen;
pub use rides::RidesScreen;
pub use route_menu::RouteMenuScreen;
pub use route_overview::RouteOverviewScreen;
pub use route_received::{RouteReceivedScreen, RouteUpdatedScreen};
pub use route_swap::RouteSwapScreen;
pub use settings::{
    AddFieldScreen, BikeTypeScreen, BluetoothScreen, DateTimeScreen, DisplayScreen, PowerScreen, ResetScreen,
    SettingsScreen, StatFieldsScreen, StatsScreen, UnitsScreen,
};
pub use statistics::StatisticsScreen;
pub use warning::{WarningFlags, WarningScreen};

/// Maximum overlay depth. Sized with headroom; the real flow never nests more than a few deep.
pub const MAX_DEPTH: usize = 8;

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

/// Apply a [`Transition`] to the stack. The root is never popped, so `back`
/// always has a defined target and the stack can never empty.
pub fn apply(stack: &mut Stack, t: Transition) {
    match t {
        Transition::None => {}
        Transition::Push(s) => {
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
    /// The loaded map's routing-profile names (routing-v2 N5) — the Bike-type settings screen cycles
    /// [`Settings::bike_profile_idx`](crate::Settings) within [`NavProfiles::len`](crate::NavProfiles).
    /// Empty before a map load / on a router-less image (the setting then cycles nowhere, inert).
    pub nav_profiles: &'a crate::NavProfiles,
    /// The App-owned POI-list snapshot, **read-only** here. The POI list's `Gesture::Press` reads
    /// the highlighted [`Poi`](obc_reader::Poi) out of it to hand to the detail screen — the one
    /// place `handle` reaches the draw-taken snapshot. Every other screen leaves it untouched.
    pub poi_scratch: &'a PoiScratch,
    pub now_ms: u32,
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
/// `Reader`, the reusable `MapRenderer`, and the in-flight encoder hold-progress
/// (0.0–1.0) the guarded-action confirm ring fills with.
pub struct Render<'a, 'd> {
    /// The streamed-map `Reader` — `None` when the base screen doesn't draw the map (a menu, the
    /// Statistics view, Home). Only the [`Map`](crate::screen::map) screen reads it, so a host can
    /// skip building the `Reader` (its SD style-table parse + stack spike) on a non-map frame and
    /// pass `None`. [`render_map`](crate::App::render_map) / [`render_frame`](crate::App::render_frame)
    /// always pass `Some`.
    pub reader: Option<&'a Reader<'d>>,
    pub renderer: &'a mut MapRenderer,
    pub state: &'a AppState,
    pub activity: &'a Activity,
    /// The persisted device settings (read-only here) — the riding views read
    /// [`units`](Settings::units) to caption + scale their readouts.
    pub settings: &'a Settings,
    pub routes: &'a [RouteSummary],
    /// The resident ride catalog (read-only) — the Rides screen draws its two-line rows + the
    /// hold-to-delete footer from it (epic #447, P7).
    pub rides: &'a [RideSummary],
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
    /// The climb the rider is currently on, or `None` between climbs (C3). Bundles the two things
    /// the Climb screen (C4) draws — the active [`ClimbSeg`] (base/top/gain/grade) and its resident
    /// detail [`ClimbProfile`] — behind one `Option` so a `Some` is exactly "a climb is being
    /// tracked, both are valid". `None` whenever [`Activity::active_climb`](crate::activity::Activity)
    /// is `None` (no route, off any climb), so a screen never reads a stale detail buffer.
    pub climb: Option<ActiveClimb<'a>>,
    /// The travelled-path breadcrumb (bounded RAM); the Map strokes it under the route. Empty when
    /// nothing has been recorded yet, so the Map can skip it with [`Breadcrumb::is_empty`].
    pub breadcrumb: &'a Breadcrumb,
    /// The single [`App`](crate::App)-owned POI-list snapshot buffer. Only the
    /// [`PoiList`](crate::screen::poi_list) screen touches it — it takes its static snapshot into
    /// this on the first draw with a `Reader` + fix (see [`PoiScratch`]); every other screen leaves
    /// it untouched. `&mut` because that lazy fill is the one screen write that happens at draw time.
    pub poi_scratch: &'a mut PoiScratch,
    /// Panel size in device pixels. Integer, because every screen lays out in whole pixels;
    /// the Map computes its `f32` viewport locally.
    pub w: i32,
    pub h: i32,
    pub now_ms: u32,
    /// The live wall-clock time this frame (set-point advanced by elapsed millis — see
    /// [`WallClock`](crate::WallClock)). The Home screensaver draws it as `HH:MM`; for boot-relative
    /// millis a screen uses [`now_ms`](Render::now_ms) instead.
    pub now: DateTime,
    pub hold_progress: f32,
    /// No current GPS fix this frame: no fix yet (acquiring) or the last has gone stale (lost). The
    /// riding views draw the "No GPS Fix" banner when set, and the Map suppresses the off-route pill
    /// (the match is stale). Computed by [`App::has_live_fix`](crate::App::has_live_fix).
    pub no_fix: bool,
    /// Microsecond clock for the map render's per-stage timing, passed to
    /// [`MapRenderer::render_timed`]. Hosts that don't profile pass
    /// [`NoopClock`](obc_render::NoopClock); the device passes its `Instant`-based clock. Part of the
    /// strippable render-instrumentation seam.
    pub clock: &'a dyn Clock,
    /// What the base screen's map render drew this frame, for the host's stats panel / frame log.
    /// Reset to default by the host each frame; only the [`Map`](crate::screen::map) screen writes
    /// it — every other screen leaves it untouched.
    pub stats: RenderStats,
}

impl Render<'_, '_> {
    /// The narrow live-data view the stat-field catalogue formats from — the one constructor of
    /// [`Readout`](crate::stat_fields::Readout), so `stat_fields` stays decoupled from the full
    /// draw context (and its `MapRenderer`).
    pub fn readout(&self) -> crate::stat_fields::Readout<'_> {
        crate::stat_fields::Readout {
            fix: self.state.user_fix,
            activity: self.activity,
            units: self.settings.units,
            route: self.route,
            profile: self.profile,
            climb: self.climb,
            now: self.now,
        }
    }
}

/// A screen's classification, declared **in its `screens!` table row** so it can never drift from
/// the enum. The two kinds behavior hangs off: [`Overlay`](ScreenKind::Overlay) screens composite
/// over the screen below instead of replacing the view, and [`Settings`](ScreenKind::Settings)
/// screens gate the debounced settings save
/// ([`App::take_settings_dirty`](crate::App::take_settings_dirty)). `Riding` (the live sensor
/// views) and `Nav` (Home + the menus/prompts) carry no behavior yet — they exist so every row
/// states what its screen *is*.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenKind {
    /// A live riding view (Map, Statistics) — full-screen, fed by the fix.
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

/// The one screen table. Each row is `Variant(StateType) => kind`; the macro expands it into the
/// [`Screen`] enum, the `handle`/`draw` delegation matches, and [`Screen::kind`]. **Adding a screen
/// = adding one row here** (plus its module, and a [`tick_timers`](Screen::tick_timers) arm only if
/// it has timed content) — there is no second list to keep in sync. Deliberately a dumb
/// token-pasting table, not a framework.
macro_rules! screens {
    ($( $(#[$doc:meta])* $variant:ident($state:ty) => $kind:ident, )+) => {
        /// The on-device screens. Each variant owns its typed state and forwards to that screen's
        /// inherent `handle`/`draw`. Generated by `screens!` — the variants, delegation, and
        /// per-screen [`ScreenKind`] all come from the one table.
        pub enum Screen {
            $( $(#[$doc])* $variant($state), )+
        }

        impl Screen {
            /// Handle one gesture, returning the navigation [`Transition`] it triggers.
            pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
                match self {
                    $( Screen::$variant(s) => s.handle(g, cx), )+
                }
            }

            /// Draw the screen into the frame's [`Canvas`]. The two host generics stop here: every
            /// screen below draws through `&mut impl Surface`, except the Map, which reaches the raw
            /// target via [`Canvas::split`] for its `MapRenderer` calls (and writes [`Render::stats`]).
            pub fn draw<D, F>(&self, cv: &mut Canvas<D, F>, rx: &mut Render)
            where
                D: DrawTarget,
                F: Fn(u16) -> D::Color,
            {
                match self {
                    $( Screen::$variant(s) => s.draw(cv, rx), )+
                }
            }

            /// This screen's [`ScreenKind`], exactly as declared in its `screens!` table row.
            pub fn kind(&self) -> ScreenKind {
                match self {
                    $( Screen::$variant(_) => ScreenKind::$kind, )+
                }
            }
        }
    };
}

screens! {
    Home(HomeScreen) => Nav,
    Map(MapScreen) => Riding,
    Statistics(StatisticsScreen) => Riding,
    /// The Climb view (epic #506, C4): the current climb's grade-striped elevation profile + cursor
    /// + four climb-scoped tiles. A full-screen riding view like the Map/Statistics siblings; C5
    /// wires it into the Back-cycle and the auto-switch, so nothing reaches it yet except the
    /// debug-open bench path.
    Climb(ClimbScreen) => Riding,
    /// The pause page: ride-so-far ledger + the guarded Resume / Finish / Discard rows.
    RideControl(RideControl) => Nav,
    /// The route-less start card (Menu → Map → press): "Start ride" / "Back". *Start ride* begins a
    /// tracking session with no route via [`start_ride_routeless`].
    RideStart(RideStartScreen) => Nav,
    Menu(MenuScreen) => Nav,
    /// The POIs browser's category list (Menu → POIs).
    PoiMenu(PoiMenuScreen) => Nav,
    /// One category's distance-sorted nearest-16 with live bearing arrows.
    PoiList(PoiListScreen) => Nav,
    /// A single POI's detail: full name, subtype, live bearing arrow, today's hours + open/closed.
    PoiDetail(PoiDetailScreen) => Nav,
    /// The POI "Create a route?" confirm (epic #116, R4): *Create route* records the one-shot
    /// [`NavRequest`](crate::activity::NavRequest) and swaps to the planning screen.
    NavConfirm(NavConfirmScreen) => Nav,
    /// The route-**planning** screen (#499): the spinning-needle wait while the host steps the
    /// resumable router; Back cancels (pops to the detail + rings [`App::take_nav_cancel`]). The
    /// host's answer ([`App::notify_nav_result`]) replaces it with the computed-route overview
    /// or the failure card.
    NavPlanning(NavPlanningScreen) => Nav,
    /// The route-planning failure card (epic #116, R4): the locked two-tier copy ("Too far to
    /// route here" / "Couldn't find a route."), info-only — any press/Back returns to the detail.
    NavFail(NavFailScreen) => Nav,
    RouteMenu(RouteMenuScreen) => Nav,
    /// The Rides screen (Menu → Rides): a see-and-delete list of stored rides with a hold-to-delete
    /// footer (greyed while recording, warning-red for an unsynced ride). Epic #447, P7 (#454).
    Rides(RidesScreen) => Nav,
    RouteOverview(RouteOverviewScreen) => Nav,
    RouteSwap(RouteSwapScreen) => Nav,
    /// The idle route-upload prompt (epic #447, P4): "ROUTE RECEIVED" — Start navigation / Dismiss.
    /// **Host-pushed** by [`App::notify_route_uploaded`]; auto-closes (= dismisses) after
    /// [`UPLOAD_POPUP_TIMEOUT_MS`]. Advisory — the route is already committed and in the Route menu.
    RouteReceived(RouteReceivedScreen) => Nav,
    /// The active-route-replaced info card (epic #447, P4). Adoption already happened when it
    /// opens (the app dropped the stale matcher/profile; the host reopened the geometry) — this
    /// only *tells* the rider. Dismiss on any press/Back, or the same auto-close.
    RouteUpdated(RouteUpdatedScreen) => Nav,
    /// The BLE pairing passkey card (epic #447, P2). **Host-pushed** by [`App::set_ble_status`]
    /// when the seam's passkey goes `Some`, popped when it clears. Opaque + non-dismissible.
    Passkey(PasskeyScreen) => Nav,
    /// The advisory warning card (issue #504): missing sensors / a slow (fragmented) map.
    /// **Host-pushed** by [`App::notify_warning`], coalesced, dismissed on any press.
    Warning(WarningScreen) => Nav,
    Settings(SettingsScreen) => Settings,
    DateTime(DateTimeScreen) => Settings,
    Units(UnitsScreen) => Settings,
    /// The Bike type screen: cycles the routing profile (§8.6) the planner weights edges by, by name
    /// from the loaded map (routing-v2 N5, epic #533).
    BikeType(BikeTypeScreen) => Settings,
    Stats(StatsScreen) => Settings,
    StatFields(StatFieldsScreen) => Settings,
    AddField(AddFieldScreen) => Settings,
    /// The Display screen: the Map's clock + scale-bar overlay toggles and the idle-return timeout.
    Display(DisplayScreen) => Settings,
    Power(PowerScreen) => Settings,
    /// The Bluetooth screen: radio on/off, status line, Paired row, hold-guarded Forget phone.
    Bluetooth(BluetoothScreen) => Settings,
    Reset(ResetScreen) => Settings,
}

impl Screen {
    /// Whether this screen draws *over* the one below (the stack composites it on
    /// top) rather than replacing the view — derived from [`kind`](Screen::kind).
    pub fn is_overlay(&self) -> bool {
        self.kind().is_overlay()
    }

    /// Whether this screen's `draw` would fill a live hold bar for its **current** selection/state
    /// — the guarded confirm rows (Ride control, Route swap), the *armed* factory-Reset bar, the
    /// Fields hold-to-delete footer over a deletable row, and the Route menu's hold-to-delete footer
    /// over a deletable route. A render-on-demand host uses
    /// [`App::top_wants_hold_fill`](crate::App::top_wants_hold_fill) to repaint a charging hold
    /// only when the fill would actually draw. Intentionally partial, like
    /// [`tick_timers`](Screen::tick_timers): most screens draw nothing hold-driven.
    pub(crate) fn wants_hold_fill(
        &self,
        settings: &Settings,
        state: &crate::AppState,
        activity: &Activity,
        routes: &[RouteSummary],
        rides: &[RideSummary],
    ) -> bool {
        match self {
            Screen::RideControl(s) => s.selection_is_guarded(),
            Screen::RouteSwap(s) => s.selection_is_guarded(),
            Screen::Reset(s) => s.hold_fill_active(),
            Screen::StatFields(s) => s.selection_is_deletable(settings),
            Screen::Bluetooth(s) => s.selection_is_guarded(state.ble_paired),
            Screen::RouteMenu(s) => s.selection_is_deletable(activity, routes.len()),
            Screen::Rides(s) => s.selection_is_deletable(activity, rides.len()),
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
    /// Statistics view runs its cursor spring-back + page auto-cycle off `now_ms`; the Home clock
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
    ) -> ScreenTick {
        match self {
            Screen::Statistics(s) => s.tick_timers(now_ms, settings),
            Screen::Home(s) => s.tick_timers(now, ms_to_next_minute),
            // The Map's clock overlay ticks over each minute (region-clipped to the pill), armed only
            // when the pill is visible — the setting on and not panning (the pan chevron owns the slot).
            Screen::Map(s) => s.tick_timers(now, ms_to_next_minute, w, pan_active, settings.map_clock),
            Screen::Menu(s) => s.tick_timers(now_ms),
            // The route-upload popups' 30 s auto-close deadline (epic #447, P4): the residual
            // wake keeps the event-driven host armed so the timeout-dismiss fires from warm
            // sleep; the removal itself runs in `App::advance_animations`' popup sweep.
            Screen::RouteReceived(s) => s.tick_timers(now_ms),
            Screen::RouteUpdated(s) => s.tick_timers(now_ms),
            Screen::RouteSwap(s) => s.tick_timers(now_ms),
            // The nav planning spinner (#499): free-runs at frame cadence until the host's
            // answer (or a cancel) removes the screen. The one screen that reports a dirty
            // region — the spinning needle's disc — so the multi-second plan's repaints stay
            // region-cheap (#500 follow-up).
            Screen::NavPlanning(s) => s.tick_timers(now_ms, w, h),
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

/// Height of the wood title bar. Sized for the Body-tier title with even ≈8 px padding.
pub const TITLE_BAR_H: i32 = 34;

/// Top of the list area (just below the title bar) shared by list screens.
pub const LIST_TOP: i32 = TITLE_BAR_H + 8;

/// Draw the shared screen chrome: a near-white background, a thin rounded outline, and a rounded
/// wood title bar with `title` left-aligned and `right` (a counter, a grade readout, …) right-
/// justified. `title` is left-aligned so a long right-hand readout never collides with it. Every
/// framed screen draws its header through this; the caller fills the body below [`LIST_TOP`].
///
/// This is the plain header; framed screens that want the BLE connected indicator in the right slot
/// (the menus) call [`title_frame_ble`] instead, threading the app's link state.
pub fn title_frame(cv: &mut impl Surface, w: i32, h: i32, title: &str, right: &str) {
    title_frame_ble(cv, w, h, title, right, false)
}

/// [`title_frame`] plus the BLE **connected indicator** (epic #447): when `ble_connected`, a small
/// static Bluetooth rune sits in the title bar's right slot, on the parchment glyph colour of the
/// bar text. The `right` readout is inset left of it so the two never overlap (in practice the
/// menus that show the indicator carry no right readout). Static — no animation — so it stays
/// dirty-row-cheap: it only appears/disappears on a link change, which
/// [`App::set_ble_status`](crate::App::set_ble_status) gates a repaint on.
pub fn title_frame_ble(cv: &mut impl Surface, w: i32, h: i32, title: &str, right: &str, ble_connected: bool) {
    use palette::*;
    cv.clear(PARCHMENT);
    cv.round_outline(rect(4, 4, w - 8, h - 8), 8, WOOD_LIGHT);
    cv.round(rect(4, 4, w - 8, TITLE_BAR_H), 6, WOOD);
    // Both rows vertically centered in the bar; the two y's account for the different glyph baselines.
    cv.text(title, Point::new(14, 8), Font::Body, TextAlign::Left, PARCHMENT);
    // The rune occupies the far-right slot; any `right` readout is pushed left of it so they can't
    // collide. `BLE_GLYPH_W` + a small gap is the reserved band.
    let right_x = if ble_connected {
        ble_glyph(cv, w - 14 - BLE_GLYPH_W, TITLE_BAR_H / 2 + 4, PARCHMENT);
        w - 14 - BLE_GLYPH_W - 8
    } else {
        w - 14
    };
    cv.text(right, Point::new(right_x, 10), Font::Label, TextAlign::Right, PARCHMENT);
}

/// Total width (px) the [`ble_glyph`] rune occupies, so callers can reserve its slot.
pub(crate) const BLE_GLYPH_W: i32 = 11;

/// Draw the Bluetooth "connected" rune centred vertically on `cy`, its left edge at `x`, in `color`.
///
/// The classic Bluetooth bind-rune (ᛒ): a vertical stem; from the stem's top a stroke runs to the
/// upper-right tip and back to the centre notch, mirrored from the bottom; and two crossing
/// back-strokes run from each tip to the opposite left corner — the diagonals that close the rune.
/// Hand-plotted as `line`s in the panel's own glyph idiom (like the climb triangles and POI bearing
/// arrows) rather than a font glyph, so it quantizes and reads at the device's pixel scale. Static
/// and tiny (~11×16), so painting it is cheap and it composites into a single dirty row-band.
pub(crate) fn ble_glyph(cv: &mut impl Surface, x: i32, cy: i32, color: u16) {
    let half = 8; // half-height → a 16 px-tall stem
    let (top, mid, bot) = (cy - half, cy, cy + half);
    let stem_x = x + 3; // the vertical bar, inset so the left back-strokes have room on either side
    let tip_x = x + BLE_GLYPH_W - 1; // the rightmost point of each triangle
    let left_x = x; // the two left corners the diagonals reach
    let quarter = half / 2;
    let (t, b, c) = (Point::new(stem_x, top), Point::new(stem_x, bot), Point::new(stem_x, mid));
    let up_tip = Point::new(tip_x, top + quarter);
    let lo_tip = Point::new(tip_x, bot - quarter);
    // The vertical stem.
    cv.line(t, b, color);
    // Right-hand strokes: top → upper-tip → centre, and bottom → lower-tip → centre.
    cv.line(t, up_tip, color);
    cv.line(up_tip, c, color);
    cv.line(b, lo_tip, color);
    cv.line(lo_tip, c, color);
    // The crossing diagonals to the opposite left corner — what makes it read as the ᛒ rune, not two
    // stacked chevrons. Upper tip → lower-left, lower tip → upper-left.
    cv.line(up_tip, Point::new(left_x, bot - quarter), color);
    cv.line(lo_tip, Point::new(left_x, top + quarter), color);
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
    cx.activity.start_session();
    Transition::Root(Screen::Map(MapScreen::new()))
}

/// The gestures the two riding views (Map and Statistics) bind identically: `press` pauses
/// tracking and opens the Ride-control overlay, `back-hold` opens the Menu. Each riding screen
/// calls this from its `Press | BackHold` arm.
pub(crate) fn riding_common(g: Gesture, cx: &mut Ctx) -> Transition {
    match g {
        Gesture::Press => {
            cx.activity.mode = Mode::Paused;
            Transition::Push(Screen::RideControl(RideControl::new()))
        }
        Gesture::BackHold => Transition::Push(Screen::Menu(MenuScreen::new())),
        _ => Transition::None,
    }
}

/// Draw one stat tile — a rounded pane in `bg` with an olive caption over a big ink Display value,
/// optionally prefixed by an up-triangle for climb figures (the panel font has no ↑ glyph). Shared
/// by the riding Statistics grid (tan panes) and the Fields editor (which draws the same tiles,
/// amber under the cursor). The caption+value block is vertically centred, so the taller editor
/// tiles and the chart-squeezed Statistics tiles both balance.
pub(crate) fn tile(cv: &mut impl Surface, area: Rectangle, label: &str, value: &str, arrow: bool, bg: u16) {
    use palette::*;
    let (x, y) = (area.top_left.x, area.top_left.y);
    cv.round(area, 5, bg);
    // Content block: Label caption (cap 18) + Display value (cap 26) with the same 18 px lead the
    // Statistics grid always had; centre it in whatever height the pane has.
    let cy = y + ((area.size.height as i32 - 48) / 2).max(4);
    // Caption inset less than the value so wide unit captions sit nearer the tile centre.
    cv.text(label, Point::new(x + 5, cy), Font::Label, TextAlign::Left, SUBTEXT);
    let vy = cy + 18;
    let vx = if arrow {
        // Up-triangle sized to sit alongside the Display digits.
        let ax = x + 8;
        cv.triangle(Point::new(ax, vy + 26), Point::new(ax + 13, vy + 26), Point::new(ax + 6, vy + 6), INK);
        x + 26
    } else {
        x + 8
    };
    cv.text(value, Point::new(vx, vy), Font::Display, TextAlign::Left, INK);
}

/// One stat-ledger row — olive caption on the left, the Display value right-aligned with a small
/// unit suffix (baselines shared), and an optional climb/descent triangle just left of the value
/// (`Some(true)` = up). All text sits on the parchment — no pane; that look is reserved for the
/// riding grid's live tiles. Shared by the Route overview and the Paused page.
pub(crate) fn ledger_row(
    cv: &mut impl Surface,
    w: i32,
    y: i32,
    caption: &str,
    value: &str,
    unit: &str,
    arrow: Option<bool>,
) {
    use palette::*;
    // Display cap is 26 from `y + 6`, Label cap 18 from `y + 14` — both bottom out at `y + 32`.
    cv.text(caption, Point::new(16, y + 14), Font::Label, TextAlign::Left, SUBTEXT);
    cv.text(unit, Point::new(w - 16, y + 14), Font::Label, TextAlign::Right, SUBTEXT);
    let unit_w = unit.chars().count() as i32 * Font::Label.char_width() as i32;
    let vx = w - 16 - unit_w - 6;
    cv.text(value, Point::new(vx, y + 6), Font::Display, TextAlign::Right, INK);
    if let Some(up) = arrow {
        let value_w = value.chars().count() as i32 * Font::Display.char_width() as i32;
        let ax = vx - value_w - 18;
        let (flat, tip) = if up { (y + 30, y + 12) } else { (y + 12, y + 30) };
        cv.triangle(Point::new(ax, flat), Point::new(ax + 13, flat), Point::new(ax + 6, tip), INK);
    }
}

/// Draw a centered two-line empty state — a bold `title` over a muted `hint` — the shared
/// "nothing to show yet" body the Route menu and Statistics draw under their header.
pub(crate) fn empty_state(cv: &mut impl Surface, w: i32, h: i32, title: &str, hint: &str) {
    cv.text(title, Point::new(w / 2, h / 2 - 28), Font::Body, TextAlign::Center, palette::INK);
    cv.text(hint, Point::new(w / 2, h / 2 + 8), Font::Label, TextAlign::Center, palette::SUBTEXT);
}

/// Append a cross-track distance after `prefix`, compacted to a whole large unit past the cross-
/// over so the readout stays within the panel width. Metric: `NNNm` below 1 km, `NNkm` above
/// (rounded). Imperial: `NNNft` below a mile, `NNmi` above. Shared by the Statistics header readout
/// and the Map's off-route pill.
pub(crate) fn write_off_route<const N: usize>(s: &mut heapless::String<N>, prefix: &str, d_m: u32, units: Units) {
    use crate::settings::{FT_PER_M, FT_PER_MI};
    if units.is_imperial() {
        let ft = (d_m as f32 * FT_PER_M) as u32;
        if ft >= FT_PER_MI {
            let _ = write!(s, "{prefix}{}mi", (ft + FT_PER_MI / 2) / FT_PER_MI);
        } else {
            let _ = write!(s, "{prefix}{ft}ft");
        }
    } else if d_m >= 1000 {
        let _ = write!(s, "{prefix}{}km", (d_m + 500) / 1000);
    } else {
        let _ = write!(s, "{prefix}{d_m}m");
    }
}

/// One option in a guarded-action menu (Ride control, Route swap): a static label and a
/// `guard` flag marking the irreversible options that need a hold-to-confirm instead of a
/// plain press.
pub(crate) struct MenuItem {
    pub label: &'static str,
    pub guard: bool,
}

/// Draw a selected option row's background for the guarded-action menus: a plain `AMBER` fill for
/// an instant option, or — when `guard` is set — a `PARCHMENT_SHADE` base that fills in `fill`
/// tracking `hold_progress` (0.0–1.0). The caller draws the label. A no-op for an unselected row.
pub(crate) fn confirm_row(
    cv: &mut impl Surface,
    row: Rectangle,
    selected: bool,
    guard: bool,
    hold_progress: f32,
    fill: u16,
    radius: u32,
) {
    if !selected {
        return;
    }
    if guard {
        cv.round(row, radius, palette::PARCHMENT_SHADE);
        let fill_w = (row.size.width as f32 * hold_progress.clamp(0.0, 1.0)) as i32;
        if fill_w > 0 {
            cv.round(rect(row.top_left.x, row.top_left.y, fill_w, row.size.height as i32), radius, fill);
        }
    } else {
        cv.round(row, radius, palette::AMBER);
    }
}

/// Layout of a guarded-action menu's option rows — the per-screen geometry
/// [`draw_guarded_rows`] lays [`MenuItem`]s out with. The label offsets are from the row's
/// top-left, hand-tuned per screen (the two panels frame their rows differently).
pub(crate) struct GuardedRowsGeometry {
    /// Left edge and width of every row.
    pub x: i32,
    pub w: i32,
    /// Top of the first row.
    pub top: i32,
    /// Row height and the vertical gap between rows.
    pub row_h: i32,
    pub gap: i32,
    /// The label anchor, relative to the row's top-left.
    pub label_dx: i32,
    pub label_dy: i32,
}

/// Draw a guarded-action menu's option rows (Ride control, Route swap): each [`MenuItem`] gets its
/// [`confirm_row`] background — the amber cursor, or the hold-progress fill in `fill` on a guarded
/// row — and its Body label. The caller draws its chrome (the PAUSED panel / the full-frame prompt)
/// and keeps its `handle` semantics.
pub(crate) fn draw_guarded_rows(
    cv: &mut impl Surface,
    items: &[MenuItem],
    selected: usize,
    hold_progress: f32,
    fill: u16,
    geo: GuardedRowsGeometry,
) {
    for (i, item) in items.iter().enumerate() {
        let y = geo.top + i as i32 * (geo.row_h + geo.gap);
        let row = rect(geo.x, y, geo.w, geo.row_h);
        confirm_row(cv, row, i == selected, item.guard, hold_progress, fill, 6);
        cv.text(
            item.label,
            Point::new(geo.x + geo.label_dx, y + geo.label_dy),
            Font::Body,
            TextAlign::Left,
            palette::INK,
        );
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
    /// Navy — the recorded breadcrumb (travelled path), stroked over the route and under the marker.
    /// Recessive so the trail behind reads quieter than the magenta route ahead.
    pub const BREADCRUMB: u16 = rgb565(0, 0, 170); // → (0,0,170) navy
}
