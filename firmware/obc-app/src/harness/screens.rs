//! Screen-stack tests: navigation [`Transition`]s per gesture, the guarded-action "needs a completed
//! hold" rule, the stack discipline ([`apply`]), and a render snapshot proving pausing swaps the
//! map view for the full-screen Paused page.

use crate::activity::Activity;
use crate::catalog_state::CatalogEffect;
use crate::recorder::RecorderMachine;
use crate::screen::{
    apply, test_ctx, ClimbScreen, Ctx, HomeScreen, MapScreen, MenuScreen, RideControl, RouteMenuScreen,
    RouteOverviewScreen, RouteSwapScreen, Screen, ScreenTick, Stack, StatisticsScreen, Transition,
};
use crate::{
    App, AppState, CameraMode, Gesture, Mode, PanBasis, PanTool, RecorderIntent, RouteSummary, Settings, WarningFlags,
    MAX_ROUTES,
};
use embedded_graphics::prelude::RgbColor; // for `Rgb888::r()` in the compositing snapshot
use obc_map_scene::BBox;
use obc_ports::{Button, ButtonEvent, Fix, InputClock, InputEvent};
use obc_reader::{MapTables, SliceSource};

use super::support::{build_min_obcm, build_min_obcm_profiles, keys, quiet_pass, render_120, ReplayFix};

/// Positional durable ids, `0..n` — what a catalog fed without an id column carried.
fn positional_ids(n: usize) -> Vec<crate::CatalogObjectId> {
    (0..n as crate::CatalogObjectId).collect()
}

/// The object one pass asks the store to remove, if the rider's guarded hold requested a delete.
/// The durable id is the pass's own resolve of the menu index.
fn took_route_delete(app: &mut App) -> Option<crate::CatalogObjectId> {
    match quiet_pass(app, 0).effects.catalog.take() {
        Some(CatalogEffect::RemoveObject { object, .. }) => Some(object),
        _ => None,
    }
}

/// Whether one pass owes the platform a settings write — the debounced save leaving the subtree.
fn settings_dirty(app: &mut App) -> bool {
    quiet_pass(app, 0).effects.settings.take().is_some()
}

/// A throwaway default [`Settings`] satisfying [`Ctx`]'s `&mut` borrow. The non-settings screens
/// under test never touch it, so each call leaks a fresh (non-aliasing) block — fine in a short-lived
/// test process.
fn leaked_settings() -> &'static mut Settings {
    Box::leak(Box::new(Settings::default()))
}

/// A handle [`Ctx`] over freshly-made state/activity. The Route-menu tests pass a catalog via
/// [`route_ctx`].
fn ctx<'a>(state: &'a mut AppState, activity: &'a mut Activity, recorder: &'a mut RecorderMachine) -> Ctx<'a> {
    Ctx { recorder, ..test_ctx(state, activity, leaked_settings()) }
}

fn ctx_with_nav<'a>(
    state: &'a mut AppState,
    activity: &'a mut Activity,
    recorder: &'a mut RecorderMachine,
    navigator: &'a mut crate::navigator::NavigatorMachine,
) -> Ctx<'a> {
    Ctx { recorder, navigator, ..test_ctx(state, activity, leaked_settings()) }
}

fn route_ctx_with_nav<'a>(
    state: &'a mut AppState,
    activity: &'a mut Activity,
    recorder: &'a mut RecorderMachine,
    navigator: &'a mut crate::navigator::NavigatorMachine,
    routes: &'a [RouteSummary],
) -> Ctx<'a> {
    Ctx { routes, ..ctx_with_nav(state, activity, recorder, navigator) }
}

/// A small synthetic route catalog (names + totals + a unit bbox to center on).
fn test_routes() -> [RouteSummary; 3] {
    let mk = |n: &str, d: u32, c: u32| {
        let mut name = heapless::String::<48>::new();
        let _ = name.push_str(n);
        RouteSummary {
            name,
            distance_km: d,
            climb_m: c,
            bbox: BBox { min_lon: 0, min_lat: 0, max_lon: 1000, max_lat: 1000 },
            start_lon: 100,
            start_lat: 100,
        }
    };
    [mk("Alpha", 10, 100), mk("Beta", 20, 200), mk("Gamma", 30, 300)]
}

// Per-gesture navigation transitions.

#[test]
fn map_press_pauses_into_ride_control() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    rec.test_open(); // the riding map (tracking) — press pauses; a browse map would open the start card
    let t = MapScreen::new().handle(Gesture::Press, &mut ctx(&mut st, &mut act, &mut rec));
    assert!(matches!(t, Transition::Push(Screen::RideControl(_))));
    assert_eq!(act.mode, Mode::Paused, "pausing stops tracking immediately");
}

#[test]
fn map_turn_zooms_in_place() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    let z0 = st.zoom;
    let t = MapScreen::new().handle(Gesture::Step(2), &mut ctx(&mut st, &mut act, &mut rec));
    assert!(matches!(t, Transition::None));
    assert!(st.zoom > z0, "a Down step zooms in");
}

/// Map zoom is `×ZOOM_STEP` per step, compounding — pins the per-step multiply so a regression
/// to an additive step is caught.
#[test]
fn map_turn_multiplies_zoom_per_step() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    MapScreen::new().handle(Gesture::Step(1), &mut ctx(&mut st, &mut act, &mut rec));
    let one = st.zoom;
    assert!(one > 1.0, "one step zooms in past 1.0, got {one}");
    MapScreen::new().handle(Gesture::Step(1), &mut ctx(&mut st, &mut act, &mut rec));
    // The second step multiplies again: zoom/one == one/1.0 (a constant ratio per step).
    assert!((st.zoom / one - one).abs() < 1e-3, "each step is the same ×ratio, got {} then {}", one, st.zoom);
}

/// A huge forward step saturates at `MAX_ZOOM` instead of overflowing to `inf` (a `Step(1000)` would
/// multiply `1.2^1000` straight to infinity).
#[test]
fn map_turn_saturates_at_max_zoom() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    MapScreen::new().handle(Gesture::Step(1000), &mut ctx(&mut st, &mut act, &mut rec));
    let saturated = st.zoom;
    assert!(saturated.is_finite(), "a huge step must clamp, not overflow to inf, got {saturated}");
    // A second huge step can't push it any higher — it's pinned at the cap.
    MapScreen::new().handle(Gesture::Step(1000), &mut ctx(&mut st, &mut act, &mut rec));
    assert_eq!(st.zoom, saturated, "already at MAX_ZOOM — further zoom-in is a no-op");
}

/// A huge backward step saturates at `MIN_ZOOM` instead of underflowing toward 0 (which would invert
/// / blank the view).
#[test]
fn map_turn_saturates_at_min_zoom() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    MapScreen::new().handle(Gesture::Step(-1000), &mut ctx(&mut st, &mut act, &mut rec));
    let saturated = st.zoom;
    assert!(saturated > 0.0, "min-zoom clamp keeps the scale positive, got {saturated}");
    MapScreen::new().handle(Gesture::Step(-1000), &mut ctx(&mut st, &mut act, &mut rec));
    assert_eq!(st.zoom, saturated, "already at MIN_ZOOM — further zoom-out is a no-op");
}

/// **The ride context is declared by all four riding views, the Up-ahead context by the timeline,
/// and nothing else declares anything** (#1515 D3/D4a). It is the one declaration a screen makes;
/// everything the sheet then does is the generic drawer's.
#[test]
fn exactly_the_riding_views_and_the_timeline_declare_a_context() {
    let declared = |s: &Screen| s.context().is_some();
    assert!(declared(&Screen::Map(MapScreen::new())));
    assert!(declared(&Screen::Statistics(StatisticsScreen::new())));
    assert!(declared(&Screen::Climb(ClimbScreen::new())));
    assert!(declared(&Screen::RideControl(RideControl::new())));
    // D4a's one addition, and it is a *different* table: the timeline's two scope controls, not
    // the ride's four actions.
    assert!(declared(&Screen::UpAhead(crate::screen::UpAheadScreen::new(0))));
    assert_ne!(
        Screen::UpAhead(crate::screen::UpAheadScreen::new(0)).context().map(|m| m.rows.len()),
        Screen::Map(MapScreen::new()).context().map(|m| m.rows.len()),
        "the timeline declares its own table, not the ride's"
    );
    // …and a representative of every other family declares nothing, so the chord does nothing.
    assert!(!declared(&Screen::Home(HomeScreen::new())));
    assert!(!declared(&Screen::Menu(MenuScreen::new())));
    assert!(!declared(&Screen::RouteMenu(RouteMenuScreen::new())));
    assert!(!declared(&Screen::Settings(crate::screen::SettingsScreen::new())));
    assert!(!declared(&Screen::PoiMenu(crate::screen::PoiMenuScreen::new())));
    assert!(!declared(&Screen::Detour(crate::screen::DetourScreen::new(&crate::navigator::RouteState::new(),))));
}

/// The chord opens the sheet over a riding view and does nothing anywhere else — the "unsupported
/// screens must not show an empty drawer" rule, driven through the one drawer owner.
#[test]
fn the_context_chord_opens_a_sheet_only_where_content_is_declared() {
    let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
    assert!(app.apply_chord(crate::input::Chord::Context), "the riding Map declares a context");
    assert!(matches!(app.top_screen(), Screen::ContextDrawer(_)));
    assert!(app.apply_chord(crate::input::Chord::Context), "the same chord closes it");
    assert!(matches!(app.top_screen(), Screen::Map(_)));

    apply(&mut app.ui.stack, Transition::Push(Screen::Menu(MenuScreen::new())));
    assert!(!app.apply_chord(crate::input::Chord::Context), "the Menu declares nothing");
    assert!(matches!(app.top_screen(), Screen::Menu(_)), "and nothing was pushed over it");
}

/// **Mutual exclusion**, both ways: one sheet at a time, and the other chord swaps rather than
/// stacks. The base underneath is untouched either way.
#[test]
fn the_two_drawers_swap_rather_than_stack() {
    let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
    let depth = app.ui.stack.len();
    assert!(app.apply_chord(crate::input::Chord::Quick));
    assert!(app.apply_chord(crate::input::Chord::Context), "the context chord takes the quick sheet off");
    assert!(matches!(app.top_screen(), Screen::ContextDrawer(_)));
    assert_eq!(app.ui.stack.len(), depth + 1, "one sheet, not two");
    assert!(app.apply_chord(crate::input::Chord::Quick), "and back the other way");
    assert!(matches!(app.top_screen(), Screen::QuickDrawer(_)));
    assert_eq!(app.ui.stack.len(), depth + 1);
    assert!(matches!(app.ui.stack.first(), Some(Screen::Home(_))));
}

/// Opening the sheet from a paused ride neither resumes nor re-pauses the session, exactly as the
/// compass menu it replaced did not.
#[test]
fn the_ride_context_opens_over_a_paused_ride_without_touching_the_session() {
    let mut app = App::new(AppState::new(0, 0, 1.0));
    app.test_start_ride();
    apply(&mut app.ui.stack, Transition::Push(Screen::RideControl(RideControl::new())));
    app.activity.mode = Mode::Paused;
    let session = app.ride_session();
    assert!(app.apply_chord(crate::input::Chord::Context));
    assert!(matches!(app.top_screen(), Screen::ContextDrawer(_)));
    assert_eq!(app.activity.mode, Mode::Paused);
    assert!(app.recording(), "the sheet opened over an open ride and left it open");
    assert_eq!(app.ride_session(), session, "…on the same session");
}

/// Whole-App ride-chrome path: Map -> the context sheet -> press **Up ahead** (epic #946, U3); row
/// gestures preserve the tracking session/mode, and Back returns to the riding view the rider
/// squeezed from — **one** Back, because the row replaced the sheet rather than stacking over it.
/// Also pins the **corridor-snapshot lifecycle** the screen drives through the App: armed while the
/// timeline is up, disarmed the moment it isn't.
#[test]
fn the_up_ahead_row_preserves_the_session_and_one_back_returns_to_the_map() {
    let mut app = App::new(AppState::new(0, 0, 1.0));
    app.test_mount_store();
    app.test_start_ride();
    app.navigator.route_state_mut().progress_m = 1_500;
    let session = app.ride_session();
    assert_eq!(app.activity.mode, Mode::Riding);
    assert!(!app.corridor_snapshot_pending(), "nothing asks for a corridor query on the map");

    assert!(app.apply_chord(crate::input::Chord::Context));
    assert!(matches!(app.top_screen(), Screen::ContextDrawer(_)));
    app.apply_gesture(Gesture::Press); // the first row = Up ahead
    assert!(matches!(app.top_screen(), Screen::UpAhead(_)));
    assert!(app.corridor_snapshot_pending(), "entering arms the snapshot (and asks for the Reader)");

    // Row gestures are in-screen: they never move the stack or the session. `Hold` is among them
    // because it is **inert** since D4a — the filter it used to open is a sheet row now, and a hold
    // that opened a mode here would show up as a screen the loop did not expect.
    for g in [Gesture::Step(1), Gesture::Press, Gesture::Hold, Gesture::Step(1), Gesture::Press] {
        app.apply_gesture(g);
        assert!(matches!(app.top_screen(), Screen::UpAhead(_)), "{g:?} stays on the timeline");
        assert_eq!(app.activity.mode, Mode::Riding);
        assert_eq!(app.ride_session(), session);
    }
    assert!(app.corridor_snapshot_pending(), "the timeline is still armed");

    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::Map(_)), "one Back: the row replaced the sheet");
    assert!(!app.corridor_snapshot_pending(), "leaving the timeline stops asking for the Reader");
    assert_eq!(app.activity.mode, Mode::Riding);
    assert_eq!(app.ride_session(), session);
}

/// The **source-scope arming rule** (epic #946, U4), end to end through the App: the Ride-settings
/// value is read when the context row opens the timeline, and a rider who asked for *waypoints only*
/// never arms the corridor snapshot at all — so the board is never asked to build a map `Reader`
/// for a query whose rows the list would refuse to draw. The other two scopes arm it as U3 did.
#[test]
fn the_up_ahead_source_setting_decides_whether_the_corridor_is_armed() {
    use crate::settings::UpAheadSource;

    let open_timeline = |source| {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.test_mount_store();
        app.set_settings(Settings { up_ahead_source: source, ..Settings::default() });
        app.test_start_ride();
        app.navigator.route_state_mut().progress_m = 1_500;
        assert!(app.apply_chord(crate::input::Chord::Context)); // Map → the ride context sheet
        app.apply_gesture(Gesture::Press); // the first row = Up ahead
        assert!(matches!(app.top_screen(), Screen::UpAhead(_)), "{source:?} still opens the timeline");
        app
    };

    let mut quiet = open_timeline(UpAheadSource::WaypointsOnly);
    assert!(!quiet.corridor_snapshot_pending(), "Waypoints only never arms the query");
    assert!(!quiet.base_needs_reader(), "…so the reader-build seam stays quiet on the timeline");
    // Not even the sheet's Filter row turns it on: a category filter scopes rows, it doesn't add a source.
    quiet.apply_gesture(Gesture::Hold);
    quiet.apply_gesture(Gesture::Step(1));
    quiet.apply_gesture(Gesture::Press);
    assert!(!quiet.corridor_snapshot_pending(), "a filter change under Waypoints only re-queries nothing");

    for source in [UpAheadSource::Both, UpAheadSource::MapPoisOnly] {
        let app = open_timeline(source);
        assert!(app.corridor_snapshot_pending(), "{source:?} arms the snapshot on entry");
        assert!(app.base_needs_reader(), "{source:?} keeps the Reader built until the query lands");
    }
}

#[test]
fn ride_control_resume_is_a_press_that_pops() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Paused));
    let mut rc = RideControl::new(); // starts on Resume
    assert!(!rc.selection_is_guarded());
    let t = rc.handle(Gesture::Press, &mut ctx(&mut st, &mut act, &mut rec));
    assert!(matches!(t, Transition::Pop), "Resume returns to the caller");
    assert_eq!(act.mode, Mode::Riding);
}

#[test]
fn ride_control_back_resumes() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Paused));
    let t = RideControl::new().handle(Gesture::Back, &mut ctx(&mut st, &mut act, &mut rec));
    assert!(matches!(t, Transition::Pop));
    assert_eq!(act.mode, Mode::Riding, "back cancels the pause");
}

#[test]
fn guarded_action_needs_a_completed_hold_not_a_press() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Paused));
    let mut rc = RideControl::new();
    rc.handle(Gesture::Step(1), &mut ctx(&mut st, &mut act, &mut rec)); // move to Finish (guarded)
    assert!(rc.selection_is_guarded());

    // A press must NOT commit an irreversible action.
    let t = rc.handle(Gesture::Press, &mut ctx(&mut st, &mut act, &mut rec));
    assert!(matches!(t, Transition::None));
    assert_eq!(act.mode, Mode::Paused, "a stray press can't finish the ride");

    // A completed hold (the recognizer only emits `Hold` once the threshold is
    // crossed) is what confirms it.
    let t = rc.handle(Gesture::Hold, &mut ctx(&mut st, &mut act, &mut rec));
    assert!(matches!(t, Transition::Home), "Finish clears back to Home");
    assert_eq!(act.mode, Mode::Idle);
}

#[test]
fn hold_on_a_non_guarded_item_does_nothing() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Paused));
    let t = RideControl::new().handle(Gesture::Hold, &mut ctx(&mut st, &mut act, &mut rec)); // on Resume
    assert!(matches!(t, Transition::None));
    assert_eq!(act.mode, Mode::Paused);
}

#[test]
fn menu_back_returns_to_caller() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    let t = MenuScreen::new().handle(Gesture::Back, &mut ctx(&mut st, &mut act, &mut rec));
    assert!(matches!(t, Transition::Pop));
}

/// The Menu's compass-needle sweep contract: a step arms a per-frame wake, the sweep converges in
/// well under a second of ticks, and a settled menu is [`ScreenTick::idle`] — so a resting menu
/// costs the event-driven host no timed repaints (the invariant
/// `ms_until_next_wake_reports_the_home_minute_then_none_on_a_static_menu` also leans on).
#[test]
fn menu_needle_sweep_arms_then_settles() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    let mut m = MenuScreen::new();
    assert_eq!(m.tick_timers(0), ScreenTick::idle(), "a fresh menu has no animation pending");

    m.handle(Gesture::Step(1), &mut ctx(&mut st, &mut act, &mut rec));
    let t0 = m.tick_timers(1_000);
    assert!(t0.next_wake_ms.is_some(), "a step puts the sweep in flight");

    let mut now = 1_000;
    let mut settled = false;
    for _ in 0..60 {
        now += 16;
        let t = m.tick_timers(now);
        if t.next_wake_ms.is_none() {
            assert!(t.changed, "the landing tick still repaints (the final snap to target)");
            settled = true;
            break;
        }
    }
    assert!(settled, "the sweep converges within 60 frames (~1 s)");
    assert_eq!(m.tick_timers(now + 16), ScreenTick::idle(), "after landing the menu is idle again");
}

// The Home → Menu → Route menu → Map flow.

#[test]
fn home_press_opens_the_menu_and_a_step_is_ignored() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    let p = HomeScreen::new().handle(Gesture::Press, &mut ctx(&mut st, &mut act, &mut rec));
    assert!(matches!(p, Transition::Push(Screen::Menu(_))), "press opens the Menu");
    let t = HomeScreen::new().handle(Gesture::Step(1), &mut ctx(&mut st, &mut act, &mut rec));
    assert!(matches!(t, Transition::None), "Up/Down steps on Home are ignored");
    // Back-hold also reaches the Menu from Home, but through the global escape rather than through
    // this screen — see `the_global_escape_reaches_the_menu_from_every_family`.
}

#[test]
fn menu_routes_station_opens_the_route_menu() {
    // The Route menu is reached from the Menu's Routes station (selected 0 by default).
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    let t = MenuScreen::new().handle(Gesture::Press, &mut ctx(&mut st, &mut act, &mut rec));
    assert!(matches!(t, Transition::Push(Screen::RouteMenu(_))), "Menu → Routes → Route menu");
}

#[test]
fn route_menu_press_opens_the_overview_and_preloads_the_route() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    let routes = test_routes();
    let mut navigator = crate::navigator::NavigatorMachine::new();
    let mut rm = RouteMenuScreen::new();
    rm.handle(Gesture::Step(1), &mut route_ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator, &routes)); // highlight route 1
    let t = rm.handle(Gesture::Press, &mut route_ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator, &routes));
    assert!(matches!(t, Transition::Push(Screen::RouteOverview(_))), "picking opens the overview");
    assert_eq!(
        navigator.route_state().active_route,
        Some(1),
        "the pick preloads the route so the overview gets a profile"
    );
    assert_eq!(act.mode, Mode::Idle, "no riding yet — the overview's START does that");
    assert!(!rec.recording(), "and no session either");
}

#[test]
fn overview_start_begins_the_session_and_opens_the_map() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    st.mode = CameraMode::Free; // the map-viewer default; starting must flip to Follow
    st.heading_up = false;
    let mut navigator = crate::navigator::NavigatorMachine::new();
    navigator.set_active_route(Some(1)); // the Route menu preloaded the preview
    let routes = test_routes();
    let t = RouteOverviewScreen::new(1, None)
        .handle(Gesture::Press, &mut route_ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator, &routes));
    assert!(matches!(t, Transition::Root(Screen::Map(_))), "starting lands on a clean [Home, Map]");
    assert_eq!(act.mode, Mode::Riding, "START begins tracking");
    assert_eq!(navigator.route_state().active_route, Some(1), "the previewed route is the active one");
    assert_eq!(rec.test_take_intent(), Some(RecorderIntent::Start), "START names the ride to Recorder");
    // Starting drops into the riding view: follow + heading-up, seeded at the start.
    assert_eq!(st.mode, CameraMode::Follow);
    assert!(st.heading_up);
    assert_eq!((st.cam_lon, st.cam_lat), (100, 100), "camera seeded at the route start");
    assert!(st.zoom > 0.2 && st.zoom < 0.25, "~0.5 m/px riding zoom, got {}", st.zoom);
}

#[test]
fn overview_back_cancels_and_restores_the_previous_route() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    let mut navigator = crate::navigator::NavigatorMachine::new();
    navigator.set_active_route(Some(2)); // the Route menu preloaded the preview…
    let routes = test_routes();
    // …over a previously loaded route 0, which back must put back.
    let t = RouteOverviewScreen::new(2, Some(0))
        .handle(Gesture::Back, &mut route_ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator, &routes));
    assert!(matches!(t, Transition::Pop), "back returns to the Route menu");
    assert_eq!(navigator.route_state().active_route, Some(0), "the previous route is restored");
    assert!(!rec.recording(), "browsing started nothing");
}

#[test]
fn route_menu_back_returns_to_caller() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    let t = RouteMenuScreen::new().handle(Gesture::Back, &mut ctx(&mut st, &mut act, &mut rec));
    assert!(matches!(t, Transition::Pop));
}

#[test]
fn route_menu_with_no_routes_ignores_press() {
    // An empty catalog: press/step are no-ops, so a routeless device can't "load" one.
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    let mut navigator = crate::navigator::NavigatorMachine::new();
    let t =
        RouteMenuScreen::new().handle(Gesture::Press, &mut ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator));
    assert!(matches!(t, Transition::None));
    assert_eq!(navigator.route_state().active_route, None);
}

// Loading a route mid-session: the swap / save prompt, Finish / Discard.

/// An activity navigating route `r`, with `rec` already recording.
fn tracking(r: usize, rec: &mut RecorderMachine) -> (Activity, crate::navigator::NavigatorMachine) {
    let act = Activity::new(Mode::Riding);
    let mut navigator = crate::navigator::NavigatorMachine::new();
    navigator.set_active_route(Some(r));
    rec.test_open();
    (act, navigator)
}

#[test]
fn loading_a_different_route_mid_session_prompts() {
    let mut rec = RecorderMachine::new();
    let mut st = AppState::new(0, 0, 1.0);
    let (mut act, mut navigator) = tracking(0, &mut rec);
    let routes = test_routes();
    let mut rm = RouteMenuScreen::new();
    rm.handle(Gesture::Step(1), &mut route_ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator, &routes)); // highlight route 1
    let t = rm.handle(Gesture::Press, &mut route_ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator, &routes));
    assert!(matches!(t, Transition::Push(Screen::RouteSwap(_))), "a different route mid-ride asks");
    assert_eq!(navigator.route_state().active_route, Some(0), "the prompt hasn't changed the route yet");
}

#[test]
fn reselecting_the_active_route_mid_session_returns_to_the_map() {
    let mut rec = RecorderMachine::new();
    let mut st = AppState::new(0, 0, 1.0);
    let (mut act, mut navigator) = tracking(1, &mut rec);
    let routes = test_routes();
    let mut rm = RouteMenuScreen::new();
    rm.handle(Gesture::Step(1), &mut route_ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator, &routes)); // highlight the active route 1
    let t = rm.handle(Gesture::Press, &mut route_ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator, &routes));
    assert!(matches!(t, Transition::Root(Screen::Map(_))), "re-picking the active route just rides it");
}

#[test]
fn route_swap_swap_only_keeps_the_session() {
    let mut rec = RecorderMachine::new();
    let mut st = AppState::new(0, 0, 1.0);
    let (mut act, mut navigator) = tracking(0, &mut rec);
    let before = rec.session();
    let routes = test_routes();
    // Default selection (0) is "Swap route".
    let t = RouteSwapScreen::new(2)
        .handle(Gesture::Press, &mut route_ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator, &routes));
    assert!(matches!(t, Transition::Root(Screen::Map(_))));
    assert_eq!(navigator.route_state().active_route, Some(2), "navigation swapped to the picked route");
    assert_eq!(rec.session(), before, "the tracking session continues unchanged");
    assert!(rec.test_take_intent().is_none(), "swap-only saves nothing");
}

#[test]
fn route_swap_save_and_new_saves_then_starts_a_fresh_session() {
    let mut rec = RecorderMachine::new();
    let mut st = AppState::new(0, 0, 1.0);
    let (mut act, mut navigator) = tracking(0, &mut rec);
    let before = rec.session();
    let routes = test_routes();
    let mut rs = RouteSwapScreen::new(2);
    rs.handle(Gesture::Step(1), &mut route_ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator, &routes)); // highlight "Save & new"
    assert!(rs.selection_is_guarded());
    // A press must not commit the guarded option — only a completed hold.
    let t = rs.handle(Gesture::Press, &mut route_ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator, &routes));
    assert!(matches!(t, Transition::None), "a press can't confirm Save & new");
    assert!(rec.test_take_intent().is_none());

    let t = rs.handle(Gesture::Hold, &mut route_ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator, &routes));
    assert!(matches!(t, Transition::Root(Screen::Map(_))));
    assert_eq!(navigator.route_state().active_route, Some(2));
    assert_eq!(rec.session(), before, "the old ride is still open until the store answers for it");
    assert_eq!(rec.test_take_intent(), Some(RecorderIntent::Save), "and the old ride is what is saved");
}

#[test]
fn ride_control_finish_saves_and_discard_discards() {
    // Finish (row 1) → save the ride.
    let mut rec = RecorderMachine::new();
    let mut st = AppState::new(0, 0, 1.0);
    let (mut act, mut navigator) = tracking(0, &mut rec);
    act.mode = Mode::Paused;
    let mut rc = RideControl::new();
    rc.handle(Gesture::Step(1), &mut ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator)); // → Finish
    let t = rc.handle(Gesture::Hold, &mut ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator));
    assert!(matches!(t, Transition::Home));
    assert_eq!(act.mode, Mode::Idle);
    assert_eq!(navigator.route_state().active_route, None);
    assert!(rec.recording(), "the ride is open until the store answers for the close");
    assert_eq!(rec.test_take_intent(), Some(RecorderIntent::Save), "Finish names the save to Recorder");

    // Discard (row 2) → throw the ride away.
    let mut rec = RecorderMachine::new();
    let mut st = AppState::new(0, 0, 1.0);
    let (mut act, mut navigator) = tracking(0, &mut rec);
    act.mode = Mode::Paused;
    let mut rc = RideControl::new();
    rc.handle(Gesture::Step(2), &mut ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator)); // → Discard
    let t = rc.handle(Gesture::Hold, &mut ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator));
    assert!(matches!(t, Transition::Home));
    assert!(rec.recording(), "the ride is open until the store answers for the close");
    assert_eq!(rec.test_take_intent(), Some(RecorderIntent::Discard));
}

#[test]
fn list_window_keeps_the_selection_visible() {
    use crate::screen::vocab::list::window_start;
    // Everything fits → never scrolls.
    assert_eq!(window_start(0, 4, 3), 0);
    assert_eq!(window_start(2, 4, 3), 0);
    // Within the first page → pinned to the top.
    assert_eq!(window_start(0, 4, 7), 0);
    assert_eq!(window_start(3, 4, 7), 0);
    // Past the page → the window follows, selection on the last visible row.
    assert_eq!(window_start(4, 4, 7), 1);
    assert_eq!(window_start(5, 4, 7), 2);
    // Clamped at the last page — can't scroll past the end.
    assert_eq!(window_start(6, 4, 7), 3);
}

#[test]
fn boot_flow_walks_home_to_route_menu_to_riding_map() {
    // End to end through `App`: Idle Home → press → Menu → press (Routes) → Route menu → press →
    // overview → press → Map.
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    app.test_mount_store();
    app.set_routes_with_ids(&test_routes(), &IDS3);
    assert_eq!(app.mode(), Mode::Idle);
    press(&mut app); // Home → Menu (Routes selected)
    assert_eq!(app.mode(), Mode::Idle, "opening the menu doesn't start riding yet");
    press(&mut app); // Menu → Route menu
    assert_eq!(app.mode(), Mode::Idle, "opening the route list doesn't start riding yet");
    press(&mut app); // Route menu → Route overview (route preloads, still not riding)
    assert_eq!(app.mode(), Mode::Idle, "the overview previews; START is what rides");
    assert_eq!(app.active_route_index(), Some(0), "the preview loads the route");
    press(&mut app); // START RIDE → Map
    assert_eq!(app.mode(), Mode::Riding);
    assert_eq!(app.active_route_index(), Some(0));
}

// Route catalog capacity: the catalog feed truncates a host store larger than the resident catalog
// (`MAX_ROUTES = 64`). A full SD card hits this; an off-by-one or missing `.take` would overflow the
// fixed `heapless::Vec`.

/// Build `n` distinctly-named route summaries (`R0`, `R1`, …) so the survivors are identifiable.
fn many_routes(n: usize) -> Vec<RouteSummary> {
    (0..n)
        .map(|i| {
            let mut name = heapless::String::<48>::new();
            let _ = core::fmt::Write::write_fmt(&mut name, format_args!("R{i}"));
            RouteSummary {
                name,
                distance_km: i as u32,
                climb_m: 0,
                bbox: BBox { min_lon: 0, min_lat: 0, max_lon: 1, max_lat: 1 },
                start_lon: 0,
                start_lat: 0,
            }
        })
        .collect()
}

/// A store larger than `MAX_ROUTES` is truncated to the first `MAX_ROUTES` in order, not overflowed.
#[test]
fn the_catalog_feed_truncates_at_max_routes() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.test_mount_store();
    let routes = many_routes(MAX_ROUTES + 50);
    app.set_routes_with_ids(&routes, &positional_ids(routes.len())); // 114 routes — well over the cap
    assert_eq!(app.routes().len(), MAX_ROUTES, "the catalog is capped at MAX_ROUTES, not overflowed");
    assert_eq!(app.routes()[0].name.as_str(), "R0", "the first scanned route is kept");
    assert_eq!(
        app.routes()[MAX_ROUTES - 1].name.as_str(),
        format!("R{}", MAX_ROUTES - 1),
        "the 64th route is the last kept; everything past it is dropped"
    );
}

/// Exactly `MAX_ROUTES` routes fit with none dropped — the cap is inclusive, guarding a `>=`/`>`
/// off-by-one.
#[test]
fn the_catalog_feed_keeps_exactly_max_routes() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.test_mount_store();
    let routes = many_routes(MAX_ROUTES);
    app.set_routes_with_ids(&routes, &positional_ids(routes.len()));
    assert_eq!(app.routes().len(), MAX_ROUTES, "a card with exactly 64 routes loses none");
}

/// The catalog feed replaces, not appends: a rescan of a now-emptied card leaves an empty catalog, not
/// the stale entries.
#[test]
fn the_catalog_feed_replaces_the_previous_catalog() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.test_mount_store();
    let routes = many_routes(10);
    app.set_routes_with_ids(&routes, &positional_ids(routes.len()));
    assert_eq!(app.routes().len(), 10);
    app.set_routes_with_ids(&[], &[]); // card removed / emptied
    assert!(app.routes().is_empty(), "a rescan replaces the catalog rather than appending");
}

// ==================== live catalog: identity remap across rescans (#450) ====================
//
// The catalog carries durable object ids; every held catalog index — `active_route`, an open
// Route-menu highlight, a pending swap — is remapped by id on every `set_routes_with_ids`. These
// pin the sharpest latent bug in epic #447: a rescan that inserts/removes a route must never
// silently shift which route is navigated.

/// Ids for [`test_routes`] — deliberately non-positional, so an index-as-id shortcut can't pass.
const IDS3: [crate::CatalogObjectId; 3] = [10, 20, 30];

/// The DoD case: while navigating route X, a *different* route is uploaded/deleted → the app
/// still navigates X, at its new index.
#[test]
fn rescan_keeps_active_route_on_the_same_route() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.test_mount_store();
    let routes = test_routes(); // Alpha, Beta, Gamma
    app.set_routes_with_ids(&routes, &IDS3);
    app.activate_route(1); // navigating Beta (id 20)

    // Delete Alpha: the list shrinks, Beta shifts 1 → 0 — navigation follows the identity.
    app.set_routes_with_ids(&routes[1..], &IDS3[1..]);
    assert_eq!(app.active_route_index(), Some(0), "shrunk list: the index moved with the route");
    assert_eq!(app.routes()[0].name.as_str(), "Beta");

    // An upload re-inserts Alpha ahead of it: the list grows, Beta shifts back 0 → 1.
    app.set_routes_with_ids(&routes, &IDS3);
    let active = app.active_route_index().expect("still navigating");
    assert_eq!(app.routes()[active].name.as_str(), "Beta", "grown list: still the same route");
}

/// The *navigated* route vanishing unloads navigation — `None`, never a neighbour aliased in by
/// the index shift.
#[test]
fn rescan_unloads_a_vanished_active_route() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.test_mount_store();
    let routes = test_routes();
    app.set_routes_with_ids(&routes, &IDS3);
    app.activate_route(1); // Beta
    let keep = [routes[0].clone(), routes[2].clone()]; // Beta deleted
    app.set_routes_with_ids(&keep, &[IDS3[0], IDS3[2]]);
    assert_eq!(app.active_route_index(), None, "the deleted route unloads; Gamma is not aliased in");
}

/// An open Route menu across a rescan: the highlight follows the previously-highlighted route's
/// identity to its new row.
#[test]
fn rescan_follows_the_open_route_menu_selection() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.test_mount_store();
    let routes = test_routes();
    app.set_routes_with_ids(&routes, &IDS3);
    app.apply_gesture(Gesture::Press); // Home → Menu (Routes selected)
    app.apply_gesture(Gesture::Press); // Menu → Route menu
    app.apply_gesture(Gesture::Step(1)); // highlight Beta
    app.set_routes_with_ids(&routes[1..], &IDS3[1..]); // Alpha deleted under the open menu
    app.apply_gesture(Gesture::Press); // open the highlighted route
    let active = app.active_route_index().expect("the overview loaded the highlighted route");
    assert_eq!(app.routes()[active].name.as_str(), "Beta", "the highlight followed Beta to its new row");
}

/// A vanished highlight falls back to the nearest row (clamped), never a dangling index.
#[test]
fn rescan_clamps_a_vanished_menu_selection() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.test_mount_store();
    let routes = test_routes();
    app.set_routes_with_ids(&routes, &IDS3);
    app.apply_gesture(Gesture::Press); // Home → Menu (Routes selected)
    app.apply_gesture(Gesture::Press); // Menu → Route menu
    app.apply_gesture(Gesture::Step(2)); // highlight Gamma (last row)
    app.set_routes_with_ids(&routes[..2], &IDS3[..2]); // Gamma deleted
    app.apply_gesture(Gesture::Press); // open whatever is highlighted now
    let active = app.active_route_index().expect("a clamped highlight still opens a real route");
    assert_eq!(app.routes()[active].name.as_str(), "Beta", "the highlight clamped to the last row");
}

// ==================== on-device route delete (epic #447, P6; epic #678 T3) ====================
//
// The Route overview's guarded Delete row records a delete request the host drains as the route's
// durable object id; after the delete + rescan, P3's remap keeps `active_route` + the highlight on
// the right routes. (T3 moved the hold-to-delete off the Route-menu footer onto the overview.)

/// A completed hold over the overview's Delete row records a delete request the host drains as that
/// route's **durable object id** (not its index) — the id lookup is `App`'s.
#[test]
fn hold_delete_requests_the_highlighted_route_id() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.test_mount_store();
    app.set_routes_with_ids(&test_routes(), &IDS3); // ids 10, 20, 30
    app.apply_gesture(Gesture::Press); // Home → Menu (Routes selected)
    app.apply_gesture(Gesture::Press); // Menu → Route menu
    app.apply_gesture(Gesture::Step(1)); // highlight Beta (id 20)
    app.apply_gesture(Gesture::Press); // Beta → Route overview
    assert_eq!(took_route_delete(&mut app), None, "no request until the hold completes");
    app.apply_gesture(Gesture::Hold); // hold with START selected (the entry state) — round 2: no delete
    assert_eq!(took_route_delete(&mut app), None, "a hold with START selected records nothing");
    app.apply_gesture(Gesture::Step(1)); // cursor → the Delete row
    app.apply_gesture(Gesture::Hold); // guarded hold on the selected Delete row = delete Beta
    assert_eq!(took_route_delete(&mut app), Some(20), "the hold recorded Beta's durable id, not its index");
    assert_eq!(took_route_delete(&mut app), None, "the request is consumed once");
    assert!(matches!(app.top_screen(), Screen::RouteMenu(_)), "the delete popped back to the Routes list");
}

/// The DoD case: deleting a *non-highlighted* route (the host removes it + re-feeds the catalog)
/// keeps the highlight on the same route, by identity — not on whatever slid into its old row.
#[test]
fn deleting_a_non_highlighted_route_keeps_the_highlight_by_id() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.test_mount_store();
    let routes = test_routes(); // Alpha(10), Beta(20), Gamma(30)
    app.set_routes_with_ids(&routes, &IDS3);
    app.apply_gesture(Gesture::Press); // Home → Menu (Routes selected)
    app.apply_gesture(Gesture::Press); // Menu → Route menu
    app.apply_gesture(Gesture::Step(1)); // highlight Beta (id 20)

    // Simulate the host handling a delete of Alpha (a *different* route) — remove it and rescan.
    let keep = [routes[1].clone(), routes[2].clone()];
    app.set_routes_with_ids(&keep, &[IDS3[1], IDS3[2]]); // Beta shifts 1 → 0

    // Pressing opens the highlighted route: still Beta, now at its new row.
    app.apply_gesture(Gesture::Press);
    let active = app.active_route_index().expect("the overview loaded the highlighted route");
    assert_eq!(app.routes()[active].name.as_str(), "Beta", "the highlight stayed on Beta across the delete");
}

/// Deleting the *highlighted* route moves the highlight sanely (clamped to the nearest surviving
/// row), never a dangling index — the host removed the route the menu was pointing at.
#[test]
fn deleting_the_highlighted_route_moves_the_highlight_sanely() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.test_mount_store();
    let routes = test_routes();
    app.set_routes_with_ids(&routes, &IDS3);
    app.apply_gesture(Gesture::Press); // Home → Menu (Routes selected)
    app.apply_gesture(Gesture::Press); // Menu → Route menu
    app.apply_gesture(Gesture::Step(2)); // highlight Gamma (id 30, last row)
    app.apply_gesture(Gesture::Press); // Gamma → Route overview
    app.apply_gesture(Gesture::Step(1)); // cursor → the Delete row (round 2: no hold-anywhere)
    app.apply_gesture(Gesture::Hold); // guarded hold on the selected Delete row = request its delete
    assert_eq!(took_route_delete(&mut app), Some(30));

    // The delete popped back to the Routes list; the host deletes Gamma and re-feeds the catalog,
    // so the highlight clamps to the new last row.
    app.set_routes_with_ids(&routes[..2], &IDS3[..2]);
    app.apply_gesture(Gesture::Press); // open whatever is highlighted now
    let active = app.active_route_index().expect("a clamped highlight still opens a real route");
    assert_eq!(app.routes()[active].name.as_str(), "Beta", "the highlight clamped to the surviving last row");
}

/// Ride to the Map on Alpha, then open the swap prompt for Gamma — the shared mid-ride setup for
/// the pending-swap remap cases.
fn app_with_pending_swap_on_gamma() -> App {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.test_mount_store();
    app.set_routes_with_ids(&test_routes(), &IDS3);
    app.apply_gesture(Gesture::Press); // Home → Menu (Routes selected)
    app.apply_gesture(Gesture::Press); // Menu → Route menu
    app.apply_gesture(Gesture::Press); // Alpha → overview
    app.apply_gesture(Gesture::Press); // START RIDE → Map, session running
    assert_eq!(app.mode(), Mode::Riding);
    assert_eq!(app.active_route_index(), Some(0));
    assert!(app.apply_chord(crate::input::Chord::Context)); // Map → the ride context sheet
    app.apply_gesture(Gesture::Step(3)); // → the Routes row
    app.apply_gesture(Gesture::Press); // the sheet → Route menu
    app.apply_gesture(Gesture::Step(2)); // highlight Gamma
    app.apply_gesture(Gesture::Press); // a different route mid-ride → the swap prompt
    assert!(matches!(app.top_screen(), Screen::RouteSwap(_)), "the swap prompt is up");
    app
}

/// A pending swap follows its pick's identity across a rescan: firing "Swap route" navigates the
/// route the rider picked, not whatever slid into its old index.
#[test]
fn rescan_remaps_a_pending_swap_by_identity() {
    let mut app = app_with_pending_swap_on_gamma();
    let routes = test_routes();
    let keep = [routes[0].clone(), routes[2].clone()]; // Beta deleted: Gamma shifts 2 → 1
    app.set_routes_with_ids(&keep, &[IDS3[0], IDS3[2]]);
    app.apply_gesture(Gesture::Press); // fire "Swap route"
    let active = app.active_route_index().expect("swap navigated");
    assert_eq!(app.routes()[active].name.as_str(), "Gamma", "the swap followed the picked route");
}

/// A pending swap whose pick vanished cancels — it must not navigate an aliased neighbour.
#[test]
fn rescan_cancels_a_swap_whose_pick_vanished() {
    let mut app = app_with_pending_swap_on_gamma();
    let routes = test_routes();
    app.set_routes_with_ids(&routes[..2], &IDS3[..2]); // Gamma itself deleted
    app.apply_gesture(Gesture::Press); // fire "Swap route" → cancels out
    assert!(matches!(app.top_screen(), Screen::RouteMenu(_)), "the prompt popped back to the menu");
    let active = app.active_route_index().expect("the original navigation is untouched");
    assert_eq!(app.routes()[active].name.as_str(), "Alpha", "still navigating the original route");
}

/// Feed a single Select press (down+up within the threshold) to the app.
fn press(app: &mut App) {
    let mut s = keys(&[
        InputEvent::Button(ButtonEvent::Down(Button::Select)),
        InputEvent::Button(ButtonEvent::Up(Button::Select)),
    ]);
    app.handle_input(InputClock(0), &mut s);
}

// Stack discipline.

#[test]
fn apply_pushes_pops_replaces_and_returns_home() {
    let mut stack: Stack = Stack::new();
    let _ = stack.push(Screen::Home(HomeScreen::new()));
    let _ = stack.push(Screen::Map(MapScreen::new()));

    // Overlay an Menu, then back out to the caller (Map).
    apply(&mut stack, Transition::Push(Screen::Menu(MenuScreen::new())));
    assert_eq!(stack.len(), 3);
    assert!(matches!(stack.last(), Some(Screen::Menu(_))));
    apply(&mut stack, Transition::Pop);
    assert!(matches!(stack.last(), Some(Screen::Map(_))), "Pop returns to caller");

    // Replace swaps the top without growing the stack.
    apply(&mut stack, Transition::Replace(Screen::Menu(MenuScreen::new())));
    assert_eq!(stack.len(), 2);
    assert!(matches!(stack.last(), Some(Screen::Menu(_))));

    // Home clears every overlay back to the root.
    apply(&mut stack, Transition::Home);
    assert_eq!(stack.len(), 1);
    assert!(matches!(stack.last(), Some(Screen::Home(_))));

    // The root is the guaranteed floor — Pop can't empty the stack.
    apply(&mut stack, Transition::Pop);
    assert_eq!(stack.len(), 1, "the Home root is never popped");
}

// Render snapshot: pausing swaps the riding map for the full-screen Paused page.

#[test]
fn pausing_swaps_the_map_for_the_paused_page() {
    let bytes = build_min_obcm(0xF800);
    let mut app = App::new(AppState::new(0, 0, 0.05));
    app.test_mount_store();
    app.test_start_ride(); // a tracking ride, so the map's press pauses (not the browse-map start card)

    // Riding: sample the (blue sea) backdrop at a point clear of the map chrome — the clock digits
    // end ~y28, the bottom-centre "No GPS Fix" chip band starts ~y74 on the 120px test frame, and
    // the scale bar + label own the bottom-left, so mid-right between them is bare map.
    let map = render_120(&mut app, &bytes);
    let backdrop = map.get(95, 45);

    // A press (Down+Up within the threshold) pauses into the Paused page.
    let mut press = keys(&[
        InputEvent::Button(ButtonEvent::Down(Button::Select)),
        InputEvent::Button(ButtonEvent::Up(Button::Select)),
    ]);
    app.handle_input(InputClock(0), &mut press);
    assert_eq!(app.mode(), Mode::Paused, "press paused the ride");

    // Now the same point carries the parchment Paused page, not the map.
    let paused = render_120(&mut app, &bytes);
    let page = paused.get(95, 45);
    assert_ne!(page, backdrop, "pausing replaced the view");
    assert!(page.r() > backdrop.r(), "the parchment page is lighter than the sea backdrop");
}

// Inspect/Pan mode (a Map sub-mode driven by the shared `AppState::pan`): enter/exit, the Move/Zoom
// tool toggle, separate Route/Free and Free-axis holds, route/free movement, and the camera freeze.

/// `hold` on the Follow map enters pan: the camera detaches (Free) and a pan state
/// appears. With no route, Free Vertical is the useful default and Move is active.
#[test]
fn map_hold_enters_pan_mode() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    let t = MapScreen::new().handle(Gesture::Hold, &mut ctx(&mut st, &mut act, &mut rec));
    assert!(matches!(t, Transition::None));
    let pan = st.pan.expect("hold enters pan");
    assert_eq!(pan.basis, PanBasis::Vertical);
    assert_eq!(pan.tool, PanTool::Move);
    assert_eq!(st.mode, CameraMode::Free, "the camera detaches while panning");
}

/// While panning, a fresh fix no longer recenters the frozen camera (but is still
/// recorded for the marker).
#[test]
fn pan_freezes_camera_against_fixes() {
    let (mut st, _act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    st.enter_pan(false, 0);
    st.update(&mut ReplayFix(Some(Fix::at(5000, 7000))));
    assert_eq!((st.cam_lon, st.cam_lat), (0, 0), "the frozen camera ignores the fix");
    assert_eq!(st.user_fix.map(|f| (f.lon, f.lat)), Some((7000, 5000)), "but the fix is recorded");
}

/// Inspect snapshots the live heading once. Removing the old orientation toggle must not let
/// later GPS courses rotate the map under a detached camera.
#[test]
fn pan_freezes_orientation_at_entry() {
    let mut st = AppState::new(0, 0, 1.0);
    st.heading_up = true;
    st.user_fix = Some(Fix { lat: 0, lon: 0, course: Some(90.0), speed_mps: Some(5.0) });
    st.enter_pan(false, 0);
    assert!((st.viewport(240.0, 320.0).course_rad - std::f32::consts::FRAC_PI_2).abs() < 1e-3);

    st.user_fix = Some(Fix { lat: 0, lon: 0, course: Some(180.0), speed_mps: Some(5.0) });
    assert!(
        (st.viewport(240.0, 320.0).course_rad - std::f32::consts::FRAC_PI_2).abs() < 1e-3,
        "the frozen 90° orientation ignores later heading changes"
    );
}

/// Up/Down moves the frozen camera along the active axis: a positive step on a
/// north-up map pans up (+latitude), leaving longitude alone, and reversing returns
/// to the start (within microdegree rounding).
#[test]
fn pan_turn_moves_camera_along_axis() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 4.0), Activity::new(Mode::Riding));
    st.enter_pan(false, 0); // north-up (heading_up defaults false)
    MapScreen::new().handle(Gesture::Step(1), &mut ctx(&mut st, &mut act, &mut rec));
    assert!(st.cam_lat > 0, "a positive step pans up = +latitude");
    assert_eq!(st.cam_lon, 0, "the vertical axis leaves longitude unchanged");
    MapScreen::new().handle(Gesture::Step(-1), &mut ctx(&mut st, &mut act, &mut rec));
    assert!(st.cam_lat.abs() <= 1 && st.cam_lon.abs() <= 1, "reversing returns to the start (±1 µdeg)");
}

/// `press` toggles Move ↔ Zoom in place. Up/Down can therefore change zoom without leaving
/// Inspect or moving the camera, and another tap restores the prior movement basis.
#[test]
fn pan_press_toggles_move_and_zoom() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    st.enter_pan(false, 0);
    let camera = (st.cam_lon, st.cam_lat);
    let zoom = st.zoom;

    MapScreen::new().handle(Gesture::Press, &mut ctx(&mut st, &mut act, &mut rec));
    assert_eq!(st.pan.unwrap().tool, PanTool::Zoom);
    MapScreen::new().handle(Gesture::Step(1), &mut ctx(&mut st, &mut act, &mut rec));
    assert!(st.zoom > zoom, "Down changes zoom while inspecting");
    assert_eq!((st.cam_lon, st.cam_lat), camera, "zooming keeps the detached centre fixed");

    MapScreen::new().handle(Gesture::Press, &mut ctx(&mut st, &mut act, &mut rec));
    assert_eq!(st.pan.unwrap().tool, PanTool::Move);
    assert_eq!(st.pan.unwrap().basis, PanBasis::Vertical, "the movement basis survives the tool toggle");
}

/// With a route loaded, Back-hold changes the Route/Free family while Select-hold changes only an
/// already-active Free axis, and is inert in Zoom and Route.
///
/// **The ring is where the family lives since #1515 D3** — Back-hold became the global escape, so
/// the Route/Free switch joined the tool on the tap that already changed mode.
#[test]
fn the_pan_ring_walks_route_move_free_move_and_zoom() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    let mut navigator = crate::navigator::NavigatorMachine::new();
    navigator.set_active_route(Some(0));
    navigator.route_state_mut().progress_m = 500;
    st.enter_pan(true, navigator.route_state().progress_m);
    let mode = |st: &AppState| (st.pan.unwrap().basis, st.pan.unwrap().tool);
    assert_eq!(mode(&st), (PanBasis::Route, PanTool::Move), "a routed pan opens on the route");

    // One lap of the ring.
    MapScreen::new().handle(Gesture::Press, &mut ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator));
    assert_eq!(mode(&st), (PanBasis::Vertical, PanTool::Move), "Route Move -> Free Move");
    MapScreen::new().handle(Gesture::Press, &mut ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator));
    assert_eq!(mode(&st), (PanBasis::Vertical, PanTool::Zoom), "Free Move -> Zoom");
    let zoom_state = st.pan.unwrap();
    MapScreen::new().handle(Gesture::Hold, &mut ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator));
    assert_eq!(st.pan.unwrap(), zoom_state, "Select-hold is inert in Zoom");
    MapScreen::new().handle(Gesture::Press, &mut ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator));
    assert_eq!(mode(&st), (PanBasis::Route, PanTool::Move), "Zoom -> Route Move closes the lap");

    // The axis survives the lap: pick Horizontal in Free, go round, and come back to it.
    MapScreen::new().handle(Gesture::Press, &mut ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator));
    MapScreen::new().handle(Gesture::Hold, &mut ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator));
    assert_eq!(mode(&st), (PanBasis::Horizontal, PanTool::Move), "Select-hold flips the Free axis");
    for _ in 0..3 {
        MapScreen::new().handle(Gesture::Press, &mut ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator));
    }
    assert_eq!(mode(&st), (PanBasis::Horizontal, PanTool::Move), "a lap returns to the axis in use");

    // Select-hold never leaves the Free family.
    MapScreen::new().handle(Gesture::Hold, &mut ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator));
    assert_eq!(mode(&st), (PanBasis::Vertical, PanTool::Move));
}

/// Without a route the ring is its two remaining stations — exactly the Move/Zoom toggle a
/// route-less pan had before the family joined the tap. Select-hold alternates the Free axes.
#[test]
fn the_route_less_pan_ring_is_free_move_and_zoom() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    st.enter_pan(false, 0);
    let mode = |st: &AppState| (st.pan.unwrap().basis, st.pan.unwrap().tool);
    for expected in [(PanBasis::Vertical, PanTool::Zoom), (PanBasis::Vertical, PanTool::Move)] {
        MapScreen::new().handle(Gesture::Press, &mut ctx(&mut st, &mut act, &mut rec));
        assert_eq!(mode(&st), expected, "no Route station to land on");
    }
    MapScreen::new().handle(Gesture::Hold, &mut ctx(&mut st, &mut act, &mut rec));
    assert_eq!(st.pan.unwrap().basis, PanBasis::Horizontal);
    MapScreen::new().handle(Gesture::Press, &mut ctx(&mut st, &mut act, &mut rec));
    MapScreen::new().handle(Gesture::Press, &mut ctx(&mut st, &mut act, &mut rec));
    assert_eq!(mode(&st), (PanBasis::Horizontal, PanTool::Move), "the axis survives the shorter lap too");
}

/// Route movement advances and retreats the distance cursor rather than forcing north/south or
/// east/west movement. Large turns clamp at the route ends.
#[test]
fn pan_route_steps_move_and_clamp_progress() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 4.0), Activity::new(Mode::Riding));
    let mut navigator = crate::navigator::NavigatorMachine::new();
    navigator.set_active_route(Some(0));
    navigator.route_state_mut().route_total_m = 1_000;
    navigator.route_state_mut().progress_m = 500;
    st.enter_pan(true, navigator.route_state().progress_m);

    MapScreen::new().handle(Gesture::Step(1), &mut ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator));
    assert!(st.pan.unwrap().route_progress_m > 500, "Down looks farther ahead on the route");
    MapScreen::new().handle(Gesture::Step(-10_000), &mut ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator));
    assert_eq!(st.pan.unwrap().route_progress_m, 0, "route movement clamps at the start");
    MapScreen::new().handle(Gesture::Step(10_000), &mut ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator));
    assert_eq!(st.pan.unwrap().route_progress_m, 1_000, "route movement clamps at the end");
}

/// Back tap is the reserved, one-gesture exit to Follow and implicitly recenters.
#[test]
fn pan_back_exits_and_recenters() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 4.0), Activity::new(Mode::Riding));
    st.user_fix = Some(Fix::at(5000, 7000));
    st.enter_pan(false, 0);
    MapScreen::new().handle(Gesture::Step(2), &mut ctx(&mut st, &mut act, &mut rec)); // pan away
    assert_ne!((st.cam_lon, st.cam_lat), (7000, 5000));

    let t = MapScreen::new().handle(Gesture::Back, &mut ctx(&mut st, &mut act, &mut rec));
    assert!(matches!(t, Transition::None));
    assert!(st.pan.is_none(), "Back tap exits Inspect");
    assert_eq!((st.cam_lon, st.cam_lat), (7000, 5000), "exit implicitly recenters on the fix");
    assert_eq!(st.mode, CameraMode::Follow, "exiting resumes Follow");
}

// The Bike-type setting (routing-v2 N5, #538): the whole-App loop — profiles mirrored from a real
// parsed map, the setting cycled by gesture, the debounced save fired on leaving the subtree, and
// the persisted byte surviving a simulated reboot through the shared codec both stores write.

/// Cycle the Bike type on a 4-profile map, leave Settings (the save cue fires), then "reboot":
/// encode → decode → a fresh App adopts the blob — the selected index survives. The store side is
/// a trivial file/RRAM write of exactly these bytes, so the codec round-trip *is* the reboot.
#[test]
fn bike_type_cycles_and_persists_across_reboot() {
    let bytes = build_min_obcm_profiles(0, &["Road", "Gravel", "MTB", "Touring"]);
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).expect("valid fixture");

    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    app.test_mount_store();
    // [Home]
    app.set_nav_profiles(tables.nav_profiles()); // the host's map-load mirror
    assert_eq!(app.nav_profiles().len(), 4, "all four §8.6 names resident");
    assert_eq!(app.nav_profiles().name(2), Some("MTB"));

    // Home → Menu → Settings → Ride → the Bike type row (the first row of the Ride group).
    app.apply_gesture(Gesture::BackHold); // → Menu
    app.apply_gesture(Gesture::Step(-1)); // compass: one ccw step to Settings
    app.apply_gesture(Gesture::Press); // → Settings list (Ride is the first row)
    app.apply_gesture(Gesture::Press); // → Ride screen (Bike type is the first row)
    app.apply_gesture(Gesture::Press); // → Bike type screen
    assert!(matches!(app.top_screen(), crate::Screen::BikeType(_)), "navigated to the Bike type screen");

    // Two steps: Road → Gravel → MTB.
    app.apply_gesture(Gesture::Step(1));
    app.apply_gesture(Gesture::Step(1));
    assert_eq!(app.settings().bike_profile_idx, 2, "two steps from Road land on MTB");

    // The save is debounced to leaving the settings subtree (Bike type → Ride → Settings list → Menu).
    assert!(!settings_dirty(&mut app), "no save cue while still inside Settings");
    app.apply_gesture(Gesture::Back); // Bike type → Ride
    app.apply_gesture(Gesture::Back); // Ride → Settings list
    app.apply_gesture(Gesture::Back); // → Menu (out of the subtree)
    assert!(settings_dirty(&mut app), "leaving Settings fires the debounced save");

    // Simulated reboot: the persisted blob seeds a fresh App (the boot path of both hosts).
    let blob = crate::settings::encode(app.settings());
    let restored = crate::settings::decode(&blob).expect("clean blob decodes");
    let mut app2 = App::new_idle(AppState::new(0, 0, 0.05));
    app2.test_mount_store();
    app2.set_settings(restored);
    assert_eq!(app2.settings().bike_profile_idx, 2, "the bike profile survives the reboot");
}

/// The provisional contour toggle (elevation EL10c, #1096) end to end through the App: the Display
/// row flips [`Settings::map_contours`], the flip is debounced-saved on leaving the settings subtree
/// like every other setting, and it survives the persisted blob into a fresh App — i.e. the rider's
/// #1097 A/B choice is still in force after a reboot.
///
/// **Provisional**: this test goes with the toggle.
#[test]
fn contours_toggle_persists_across_reboot() {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    app.test_mount_store();
    // [Home]
    assert!(app.settings().map_contours, "contours default on — nothing to switch on first");

    // Home → Menu → Settings → Display → the Contours row (Clock, Scale bar, Contours, Idle).
    app.apply_gesture(Gesture::BackHold); // → Menu
    app.apply_gesture(Gesture::Step(-1)); // compass: one ccw step to Settings
    app.apply_gesture(Gesture::Press); // → Settings list (Ride is the first row)
    app.apply_gesture(Gesture::Step(1)); // → the Display row
    app.apply_gesture(Gesture::Press); // → Display screen (Clock is the first row)
    assert!(matches!(app.top_screen(), crate::Screen::Display(_)), "navigated to the Display screen");
    app.apply_gesture(Gesture::Step(2)); // Clock → Scale bar → Contours
    app.apply_gesture(Gesture::Press);
    assert!(!app.settings().map_contours, "the row flipped the setting");

    // Debounced to leaving the settings subtree, exactly like the other Display toggles.
    assert!(!settings_dirty(&mut app), "no save cue while still inside Settings");
    app.apply_gesture(Gesture::Back); // Display → Settings list
    app.apply_gesture(Gesture::Back); // → Menu (out of the subtree)
    assert!(settings_dirty(&mut app), "leaving Settings fires the debounced save");

    // Simulated reboot: the persisted blob seeds a fresh App (the boot path of both hosts).
    let blob = crate::settings::encode(app.settings());
    let restored = crate::settings::decode(&blob).expect("clean blob decodes");
    let mut app2 = App::new_idle(AppState::new(0, 0, 0.05));
    app2.test_mount_store();
    app2.set_settings(restored);
    assert!(!app2.settings().map_contours, "the contour choice survives the reboot");
}

/// A stored index past the loaded map's profile count (a stale setting against a smaller map)
/// renders **profile 0's name** — the profile the router actually falls back to (N3), so the UI
/// never names a profile the map doesn't have — and an in-range index renders the map's name.
/// Pinned through the App's resident mirror, i.e. exactly what the Bike-type row and the overview
/// label draw.
#[test]
fn bike_type_out_of_range_renders_fallback() {
    let bytes = build_min_obcm_profiles(0, &["Road", "MTB"]);
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).expect("valid fixture");

    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    app.test_mount_store();
    app.set_nav_profiles(tables.nav_profiles());
    app.set_settings(Settings { bike_profile_idx: 7, ..Settings::default() }); // stale: map has 2

    let mut label: heapless::String<20> = heapless::String::new();
    app.nav_profiles().write_label(app.settings().bike_profile_idx, &mut label);
    assert_eq!(label.as_str(), "Road", "an out-of-range index shows profile 0's name — what routing will use");

    let mut ok: heapless::String<20> = heapless::String::new();
    app.nav_profiles().write_label(1, &mut ok);
    assert_eq!(ok.as_str(), "MTB", "an in-range index shows the map's name");
}

// ---- The global Back-hold escape (#1515 D3) ----------------------------------------------------

/// **Back hold always reaches the main menu.** One representative of every screen family, plus the
/// two places D2 explicitly could not leave — a drawer subpage and the power confirmation.
///
/// The mutant this kills: resolve `BackHold` after screen dispatch instead of before it, and each
/// screen's inert arm swallows it again — the exception list the acceptance criteria forbid.
#[test]
fn the_global_escape_reaches_the_menu_from_every_family() {
    /// One family: its name, and how to reach it from `[Home, Map]`.
    type Family = (&'static str, fn(&mut App));
    // Each case is (name, how to get there). The escape then has to land on the Menu.
    let families: [Family; 7] = [
        ("the Home root", |app| apply(&mut app.ui.stack, Transition::Home)),
        ("a riding view", |app| apply(&mut app.ui.stack, Transition::Root(Screen::Map(MapScreen::new())))),
        ("the paused page", |app| apply(&mut app.ui.stack, Transition::Push(Screen::RideControl(RideControl::new())))),
        ("a settings page", |app| {
            apply(&mut app.ui.stack, Transition::Push(Screen::Settings(crate::screen::SettingsScreen::new())));
            apply(&mut app.ui.stack, Transition::Push(Screen::Display(crate::screen::DisplayScreen::new())));
        }),
        ("a nav list", |app| apply(&mut app.ui.stack, Transition::Push(Screen::RouteMenu(RouteMenuScreen::new())))),
        // D2 handover 1: the quick drawer's own pages, including the power confirmation, whose
        // `BackHold` arm was `Transition::None`.
        ("the quick drawer", |app| {
            assert!(app.apply_chord(crate::input::Chord::Quick));
        }),
        ("the power confirmation", |app| {
            app.set_backlight_available(true); // the four-control row, so Power is the last one
            assert!(app.apply_chord(crate::input::Chord::Quick));
            for _ in 0..3 {
                app.apply_gesture(Gesture::Step(1)); // brightness → … → the power control
            }
            app.advance_animations(InputClock(1_000)); // settle the sheet's open slide
            app.apply_gesture(Gesture::Press); // → the guarded confirmation
            assert!(app.top_wants_hold_fill(), "the confirmation is up — the page D2 could not leave");
        }),
    ];
    for (name, reach) in families {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
        reach(&mut app);
        app.apply_gesture(Gesture::BackHold);
        assert!(matches!(app.top_screen(), Screen::Menu(_)), "the escape did not leave {name}");
        assert!(!app.ui.stack.iter().any(|s| s.is_overlay()), "a sheet survived the escape from {name}");
    }
}

/// The context sheet is a drawer subpage too, and the escape takes it with it rather than burying
/// it under the Menu.
#[test]
fn the_escape_takes_the_context_sheet_with_it() {
    let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
    assert!(app.apply_chord(crate::input::Chord::Context));
    app.apply_gesture(Gesture::BackHold);
    assert!(matches!(app.top_screen(), Screen::Menu(_)));
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::Map(_)), "Back out of the Menu lands on the base, not the sheet");
}

/// **It goes to the Menu, it never adds one.** A rider squeezing the bar repeatedly must not walk
/// the stack toward `MAX_DEPTH`.
#[test]
fn a_repeated_escape_does_not_stack_menus() {
    let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
    app.apply_gesture(Gesture::BackHold);
    let depth = app.ui.stack.len();
    for _ in 0..6 {
        app.apply_gesture(Gesture::BackHold);
    }
    assert_eq!(app.ui.stack.len(), depth, "the escape is idempotent once it has arrived");
}

/// The escape's refusal set, driven rather than read off the table: a **blocking** modal refuses
/// both the chord and the escape, and the recovered-ride card refuses only the escape — a sheet
/// over it is harmless, but leaving it would strand the recovered recording that Back already
/// cannot dismiss.
#[test]
fn a_card_the_rider_must_answer_refuses_the_escape() {
    let mut app = App::new(AppState::new(0, 0, 1.0));
    app.set_ble_status(crate::BleStatus { link: crate::BleLink::Connected, paired: false, passkey: Some(123_456) });
    assert!(matches!(app.top_screen(), Screen::Passkey(_)), "the passkey card is up");
    app.apply_gesture(Gesture::BackHold);
    assert!(matches!(app.top_screen(), Screen::Passkey(_)), "a blocking modal refuses the escape");

    // A sheet the rider opened *over* the recovery card is not consent to walk away from it: the
    // refusal is asked of the base, not of whatever is on top.
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    assert!(app.offer_damaged_ride(), "the recovery card is offered");
    assert!(app.apply_chord(crate::input::Chord::Quick), "…and a quick sheet over it is harmless");
    app.apply_gesture(Gesture::BackHold);
    assert!(
        matches!(app.top_screen(), Screen::QuickDrawer(_)),
        "a sheet the rider opened over the card is not consent to walk away from it"
    );

    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    assert!(app.offer_damaged_ride());
    app.apply_gesture(Gesture::BackHold);
    assert!(matches!(app.top_screen(), Screen::RideRecovery(_)), "the recovery decision stays put");
}

/// **The escape is bounded, and the host's card still lands** — the rider-gesture-only walk that
/// used to fill the stack.
///
/// Escape → re-descend → escape is the escape's *most ordinary* use ("get me out of six levels of
/// settings"). While it pushed unconditionally, two laps of it reached `MAX_DEPTH`, where the next
/// host-pushed card is dropped in release and panics in debug — exactly the reserve
/// [`deepest_mid_ride_settings_path_keeps_room_for_host_warning`] exists to protect, defeated by a
/// path that test does not walk. The rewind makes every lap after the first *shrink* the stack.
///
/// Driven the way a rider drives it: no `stack` surgery, only gestures.
#[test]
fn laps_of_escape_and_re_descent_leave_room_for_a_host_card() {
    let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map], riding
    app.apply_gesture(Gesture::BackHold); // the first escape pushes: [Home, Map, Menu]
    assert!(matches!(app.top_screen(), Screen::Menu(_)));

    // From the Menu down to the deepest ordinary settings page, and back out with the escape.
    let mut deepest = 0;
    for lap in 0..4 {
        if lap == 0 {
            app.apply_gesture(Gesture::Step(-1)); // Routes → the Settings station
        }
        // From lap 1 the dial is *still* on Settings: the escape rewinds to the same Menu, with
        // the station the rider last used selected. That is a property of rewinding rather than
        // pushing a fresh Menu, so it is asserted here instead of worked around.
        app.apply_gesture(Gesture::Press); // → the Settings list (Ride first)
        assert!(matches!(app.top_screen(), Screen::Settings(_)), "lap {lap}: the Menu kept its station");
        app.apply_gesture(Gesture::Press); // → Ride
        app.apply_gesture(Gesture::Step(1)); // Bike type → Data fields
        app.apply_gesture(Gesture::Press); // → Fields
        assert!(matches!(app.top_screen(), Screen::StatFields(_)), "lap {lap} reached the fields editor");
        deepest = deepest.max(app.ui.stack.len());

        app.apply_gesture(Gesture::BackHold);
        assert!(matches!(app.top_screen(), Screen::Menu(_)), "lap {lap} escaped to the Menu");
        // Idempotent once it has arrived: a second squeeze of the bar costs nothing.
        let settled = app.ui.stack.len();
        app.apply_gesture(Gesture::BackHold);
        assert_eq!(app.ui.stack.len(), settled, "lap {lap}: a second escape moved the stack");
        assert_eq!(settled, 3, "lap {lap}: the escape rewound to the Menu the rider already had");
    }
    assert!(deepest < crate::screen::MAX_DEPTH, "the reachable depth is {deepest}, at the ceiling of MAX_DEPTH");

    // The card the reserve exists for: after all that, it must still land.
    app.on_warning(WarningFlags::REC_ERROR);
    assert!(matches!(app.top_screen(), Screen::Warning(_)), "the host warning must still fit over the escape");
}

/// **A shutdown in progress is not cancellable, by either device-wide input.** The rider completed
/// the guarded hold and the host is about to call the power-off port; the terminal frame's own
/// contract is that nothing dismisses it, and that has to hold for the escape *and* for a squeeze.
///
/// It cannot be expressed in `Caps`: the frame is a **page** of the quick drawer, and a drawer must
/// never declare `blocks_chords` — that is the declaration the chord which closes it would trip
/// over. So the refusal is a runtime check in both owners, and this is its only guard.
#[test]
fn nothing_cancels_a_shutdown_already_in_progress() {
    /// A device on the terminal POWERING OFF frame, reached the way a rider reaches it.
    fn powering_off() -> App {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
                                                          // A panel with a light, so the root row is the issue's four and Power is the last of them.
        app.set_backlight_available(true);
        assert!(app.apply_chord(crate::input::Chord::Quick));
        for _ in 0..3 {
            app.apply_gesture(Gesture::Step(1)); // brightness → … → the power control
        }
        app.advance_animations(InputClock(1_000)); // settle the sheet's open slide
        app.apply_gesture(Gesture::Press); // → the guarded confirmation
        assert!(app.top_wants_hold_fill(), "the confirmation is up and drawing its hold bar");
        app.advance_animations(InputClock(2_000)); // settle the page slide
        app.apply_gesture(Gesture::Hold); // the completed hold
        assert!(app.power_off_requested(), "the host is about to call the power-off port");
        app
    }

    let mut app = powering_off();
    app.apply_gesture(Gesture::BackHold);
    assert!(app.power_off_requested(), "the global escape must not walk away from a shutdown");
    assert!(matches!(app.top_screen(), Screen::QuickDrawer(_)));

    for chord in [crate::input::Chord::Context, crate::input::Chord::Quick] {
        let mut app = powering_off();
        assert!(!app.apply_chord(chord), "{chord:?} moved something over the powering-off frame");
        assert!(app.power_off_requested(), "{chord:?} cancelled a shutdown already in progress");
    }

    // The third door, and the least obvious: the card sweep runs in the same `handle_input` that
    // applied the completed hold, so a card landing there would take the frame away — by rewriting
    // a slot under it, or by the "nothing lands on top of a drawer" rule popping the sheet. The
    // card is refused instead, and refusing is already how a card keeps its one-shot fact.
    let mut app = powering_off();
    app.on_warning(WarningFlags::REC_ERROR);
    assert!(app.power_off_requested(), "a host card must not cancel a shutdown in progress");
    assert!(matches!(app.top_screen(), Screen::QuickDrawer(_)), "the panel keeps the powering-off frame");
    // The fourth door: the remote DFU card is the one arrival that comes from neither the card
    // scheduler nor a screen transition, so it has to say this itself. Deferring is already its
    // answer to an inconvenient moment.
    let mut app = powering_off();
    assert!(!app.open_remote_dfu_check(), "a phone's install request must not open over a shutdown");
    assert!(app.power_off_requested(), "…and must not cancel it");
    assert!(matches!(app.top_screen(), Screen::QuickDrawer(_)), "the panel keeps the powering-off frame");
}

/// The same remote card is also the one arrival that used to push **raw**, stepping around the rule
/// every other one obeys: it landed on top of an open sheet instead of taking it.
#[test]
fn the_remote_dfu_card_takes_an_open_sheet_with_it() {
    for chord in [crate::input::Chord::Quick, crate::input::Chord::Context] {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
        assert!(app.apply_chord(chord));
        assert!(app.open_remote_dfu_check(), "the phone's request opens the scan wait");
        assert!(matches!(app.top_screen(), Screen::DfuCheck(_)));
        assert!(!app.debug_stack_has_overlay(), "{chord:?}: the sheet went with it");
    }
}

/// **A drawer is transient chrome: nothing lands on top of one.** A host card arriving over an open
/// sheet takes the sheet with it, so dismissing the card lands the rider on the screen they were on
/// rather than back inside a drawer they had finished with — and the escape's "any sheet goes with
/// it" rule holds for every slot, not only the top one.
#[test]
fn a_card_landing_over_a_sheet_takes_the_sheet_with_it() {
    for chord in [crate::input::Chord::Quick, crate::input::Chord::Context] {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
        assert!(app.apply_chord(chord));
        assert!(app.ui.stack.iter().any(|s| s.is_overlay()), "{chord:?} opened a sheet");

        app.on_warning(WarningFlags::REC_ERROR);
        assert!(matches!(app.top_screen(), Screen::Warning(_)), "the card landed");
        assert!(!app.ui.stack.iter().any(|s| s.is_overlay()), "{chord:?}: the sheet went with it");

        app.apply_gesture(Gesture::Press); // dismiss the card
        assert!(matches!(app.top_screen(), Screen::Map(_)), "{chord:?}: dismissing lands on the base");
    }
}
