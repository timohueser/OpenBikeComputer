//! Screen-stack tests: per-gesture transitions, the guarded-hold rule, and stack discipline.

use crate::activity::Activity;
use crate::catalog_state::CatalogEffect;
use crate::recorder::RecorderMachine;
use crate::screen::{
    apply, test_ctx, ClimbScreen, Ctx, HomeScreen, MapScreen, MenuScreen, RideControl, RouteMenuScreen,
    RouteOverviewScreen, RouteSwapScreen, Screen, ScreenTick, Stack, StatisticsScreen, Transition,
};
use crate::{
    Alert, App, AppState, CameraMode, Gesture, Mode, PanBasis, PanTool, RecorderIntent, RouteSummary, Settings,
    MAX_ROUTES,
};
use embedded_graphics::prelude::RgbColor; // for `Rgb888::r()` in the compositing snapshot
use obc_map_scene::BBox;
use obc_ports::{Button, ButtonEvent, Fix, InputClock, InputEvent};

use super::support::{build_min_obcm, keys, quiet_pass, render_120, ride_summary, ReplayFix};

fn positional_ids(n: usize) -> Vec<crate::CatalogObjectId> {
    (0..n as crate::CatalogObjectId).collect()
}

/// The durable object id one pass asks the store to remove, if a guarded hold requested a delete.
fn took_route_delete(app: &mut App) -> Option<crate::CatalogObjectId> {
    match quiet_pass(app, 0).effects.catalog.take() {
        Some(CatalogEffect::RemoveObject { object, .. }) => Some(object),
        _ => None,
    }
}

/// Whether one pass owes the platform a settings write.
fn settings_dirty(app: &mut App) -> bool {
    quiet_pass(app, 0).effects.settings.take().is_some()
}

/// A throwaway [`Settings`] to satisfy [`Ctx`]'s `&mut` borrow. Each call leaks a fresh block,
/// which is fine in a short-lived test process.
fn leaked_settings() -> &'static mut Settings {
    Box::leak(Box::new(Settings::default()))
}

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

#[test]
fn map_turn_multiplies_zoom_per_step() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    MapScreen::new().handle(Gesture::Step(1), &mut ctx(&mut st, &mut act, &mut rec));
    let one = st.zoom;
    assert!(one > 1.0, "one step zooms in past 1.0, got {one}");
    MapScreen::new().handle(Gesture::Step(1), &mut ctx(&mut st, &mut act, &mut rec));
    assert!((st.zoom / one - one).abs() < 1e-3, "each step is the same ×ratio, got {} then {}", one, st.zoom);
}

/// A huge step must clamp: `1.2^1000` would multiply straight to infinity.
#[test]
fn map_turn_saturates_at_max_zoom() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    MapScreen::new().handle(Gesture::Step(1000), &mut ctx(&mut st, &mut act, &mut rec));
    let saturated = st.zoom;
    assert!(saturated.is_finite(), "a huge step must clamp, not overflow to inf, got {saturated}");
    MapScreen::new().handle(Gesture::Step(1000), &mut ctx(&mut st, &mut act, &mut rec));
    assert_eq!(st.zoom, saturated, "already at MAX_ZOOM — further zoom-in is a no-op");
}

/// A huge backward step clamps at `MIN_ZOOM`; a scale near 0 would blank the view.
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

#[test]
fn exactly_the_riding_views_and_the_timeline_declare_a_context() {
    let declared = |s: &Screen| s.context().is_some();
    assert!(declared(&Screen::Map(MapScreen::new())));
    assert!(declared(&Screen::Statistics(StatisticsScreen::new())));
    assert!(declared(&Screen::Climb(ClimbScreen::new())));
    assert!(declared(&Screen::RideControl(RideControl::new())));
    assert!(!core::ptr::eq(
        Screen::Map(MapScreen::new()).context().unwrap(),
        Screen::Statistics(StatisticsScreen::new()).context().unwrap(),
    ));
    assert!(declared(&Screen::WhatsNext(crate::screen::WhatsNextScreen::new())));
    assert_ne!(
        Screen::WhatsNext(crate::screen::WhatsNextScreen::new()).context().map(|m| m.rows.len()),
        Screen::Map(MapScreen::new()).context().map(|m| m.rows.len()),
        "the timeline declares its own table, not the ride's"
    );

    // The one screen of the create-route flow that declares anything.
    let confirm = || crate::harness::support::selected_place();
    assert!(declared(&confirm()));
    assert!(core::ptr::eq(
        confirm().context().expect("the confirm card declares a context"),
        &crate::screen::context_drawer::ROUTE_PLAN
    ));
    assert_eq!(confirm().context().map(|m| m.rows.len()), Some(1), "one row: there is no second route option");
    // The rest of the flow declares nothing: the planner already captured the profile, and the
    // overview's BIKE TYPE row promises the profile the route was planned under.
    assert!(!declared(&Screen::NavPlanning(crate::screen::NavPlanningScreen::new("Fontaine"))));
    assert!(!declared(&Screen::RouteOverview(crate::screen::RouteOverviewScreen::computed(0, None))));
    assert!(!declared(&Screen::Home(HomeScreen::new())));
    assert!(!declared(&Screen::Menu(MenuScreen::new())));
    assert!(!declared(&Screen::RouteMenu(RouteMenuScreen::new())));
    assert!(!declared(&Screen::Settings(crate::screen::SettingsPage::hub())));
    assert!(!declared(&Screen::Detour(crate::screen::DetourScreen::new(&crate::navigator::RouteState::new(),))));
}

/// Rows 0-3 are pinned equal, label for label and action for action: a rider who squeezes on the
/// Map and one who squeezes on Statistics must reach the same four things. The display sheet is
/// declared by no screen; the only way to it is the Map's fifth row.
#[test]
fn the_map_declares_the_ride_actions_plus_its_own_display_row() {
    use crate::screen::context_drawer::{MAP, MAP_DISPLAY, RIDE};

    let table = |s: Screen| s.context().expect("a riding view declares a context");
    let map = table(Screen::Map(MapScreen::new()));
    assert!(core::ptr::eq(map, &MAP), "the Map declares its own table");
    for sibling in [
        Screen::Statistics(StatisticsScreen::new()),
        Screen::Climb(ClimbScreen::new()),
        Screen::RideControl(RideControl::new()),
    ] {
        assert!(core::ptr::eq(table(sibling), &RIDE), "the other three riding views keep the ride table");
    }

    assert_eq!(map.rows.len(), RIDE.rows.len() + 1, "Map adds its display row");
    for (m, r) in map.rows[..3].iter().zip(&RIDE.rows[..3]) {
        let lang = crate::settings::Language::En;
        assert_eq!(crate::i18n::t(m.label, lang), crate::i18n::t(r.label, lang), "the ride actions must not drift");
    }

    for screen in [
        Screen::Map(MapScreen::new()),
        Screen::Statistics(StatisticsScreen::new()),
        Screen::Climb(ClimbScreen::new()),
        Screen::RideControl(RideControl::new()),
        Screen::WhatsNext(crate::screen::WhatsNextScreen::new()),
    ] {
        assert!(!screen.context().is_some_and(|m| core::ptr::eq(m, &MAP_DISPLAY)), "no screen declares the sub-sheet");
    }
}

/// Unsupported screens must not show an empty drawer.
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

/// Map -> the context sheet -> Up ahead. Row gestures keep the tracking session and mode, and one
/// Back returns to the riding view, because the row replaced the sheet rather than stacking over it.
#[test]
fn assistant_whats_next_preserves_the_session_and_back_returns_through_the_questions() {
    let mut app = App::new(AppState::new(0, 0, 1.0));
    app.test_mount_store();
    app.test_start_ride();
    app.navigator.route_state_mut().progress_m = 1_500;
    let session = app.ride_session();
    assert!(app.apply_chord(crate::input::Chord::Context));
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::Assistant(_)));
    app.apply_gesture(Gesture::Step(1));
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::WhatsNext(_)));
    assert_eq!(app.activity.mode, Mode::Riding);
    assert_eq!(app.ride_session(), session);
    assert_eq!(app.navigator.route_state().progress_m, 1_500);
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::Assistant(_)));
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::Map(_)));
    assert_eq!(app.ride_session(), session);
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

    let t = rc.handle(Gesture::Press, &mut ctx(&mut st, &mut act, &mut rec));
    assert!(matches!(t, Transition::None));
    assert_eq!(act.mode, Mode::Paused, "a stray press can't finish the ride");

    // The recognizer only emits `Hold` once the hold threshold is crossed.
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

/// A step arms a per-frame wake and the sweep settles back to [`ScreenTick::idle`], so a resting
/// menu costs the event-driven host no timed repaints.
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

#[test]
fn home_press_opens_the_menu_and_a_step_is_ignored() {
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    let p = HomeScreen::new().handle(Gesture::Press, &mut ctx(&mut st, &mut act, &mut rec));
    assert!(matches!(p, Transition::Push(Screen::Menu(_))), "press opens the Menu");
    let t = HomeScreen::new().handle(Gesture::Step(1), &mut ctx(&mut st, &mut act, &mut rec));
    assert!(matches!(t, Transition::None), "Up/Down steps on Home are ignored");
}

#[test]
fn menu_routes_station_opens_the_route_menu() {
    // Routes is the Menu's default station.
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
    let mut rec = RecorderMachine::new();
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    let mut navigator = crate::navigator::NavigatorMachine::new();
    let t =
        RouteMenuScreen::new().handle(Gesture::Press, &mut ctx_with_nav(&mut st, &mut act, &mut rec, &mut navigator));
    assert!(matches!(t, Transition::None));
    assert_eq!(navigator.route_state().active_route, None);
}

/// An Activity in Riding mode and a Navigator following route `r`, with `rec` already recording.
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

#[test]
fn the_catalog_feed_keeps_exactly_max_routes() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.test_mount_store();
    let routes = many_routes(MAX_ROUTES);
    app.set_routes_with_ids(&routes, &positional_ids(routes.len()));
    assert_eq!(app.routes().len(), MAX_ROUTES, "a card with exactly 64 routes loses none");
}

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

// The catalog carries durable object ids: every held index — `active_route`, an open Route-menu
// highlight, a pending swap — is remapped by id on every `set_routes_with_ids`.

/// Ids for [`test_routes`] — deliberately non-positional, so an index-as-id shortcut can't pass.
const IDS3: [crate::CatalogObjectId; 3] = [10, 20, 30];

#[test]
fn rescan_keeps_active_route_on_the_same_route() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.test_mount_store();
    let routes = test_routes(); // Alpha, Beta, Gamma
    app.set_routes_with_ids(&routes, &IDS3);
    app.activate_route(1); // navigating Beta (id 20)

    // Delete Alpha: the list shrinks, Beta shifts 1 → 0.
    app.set_routes_with_ids(&routes[1..], &IDS3[1..]);
    assert_eq!(app.active_route_index(), Some(0), "shrunk list: the index moved with the route");
    assert_eq!(app.routes()[0].name.as_str(), "Beta");

    // An upload re-inserts Alpha ahead of it: the list grows, Beta shifts back 0 → 1.
    app.set_routes_with_ids(&routes, &IDS3);
    let active = app.active_route_index().expect("still navigating");
    assert_eq!(app.routes()[active].name.as_str(), "Beta", "grown list: still the same route");
}

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
    app.apply_gesture(Gesture::Hold); // hold with START selected (the entry state)
    assert_eq!(took_route_delete(&mut app), None, "a hold with START selected records nothing");
    app.apply_gesture(Gesture::Step(1)); // cursor → the Delete row
    app.apply_gesture(Gesture::Hold); // guarded hold on the selected Delete row = delete Beta
    assert_eq!(took_route_delete(&mut app), Some(20), "the hold recorded Beta's durable id, not its index");
    assert_eq!(took_route_delete(&mut app), None, "the request is consumed once");
    assert!(matches!(app.top_screen(), Screen::RouteMenu(_)), "the delete popped back to the Routes list");
}

#[test]
fn deleting_a_non_highlighted_route_keeps_the_highlight_by_id() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.test_mount_store();
    let routes = test_routes(); // Alpha(10), Beta(20), Gamma(30)
    app.set_routes_with_ids(&routes, &IDS3);
    app.apply_gesture(Gesture::Press); // Home → Menu (Routes selected)
    app.apply_gesture(Gesture::Press); // Menu → Route menu
    app.apply_gesture(Gesture::Step(1)); // highlight Beta (id 20)

    // Simulate the host handling a delete of Alpha, a different route: remove it and rescan.
    let keep = [routes[1].clone(), routes[2].clone()];
    app.set_routes_with_ids(&keep, &[IDS3[1], IDS3[2]]); // Beta shifts 1 → 0

    app.apply_gesture(Gesture::Press);
    let active = app.active_route_index().expect("the overview loaded the highlighted route");
    assert_eq!(app.routes()[active].name.as_str(), "Beta", "the highlight stayed on Beta across the delete");
}

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
    app.apply_gesture(Gesture::Step(1)); // cursor → the Delete row
    app.apply_gesture(Gesture::Hold); // guarded hold on the selected Delete row = request its delete
    assert_eq!(took_route_delete(&mut app), Some(30));

    // The host deletes Gamma and re-feeds the catalog, so the highlight clamps to the new last row.
    app.set_routes_with_ids(&routes[..2], &IDS3[..2]);
    app.apply_gesture(Gesture::Press); // open whatever is highlighted now
    let active = app.active_route_index().expect("a clamped highlight still opens a real route");
    assert_eq!(app.routes()[active].name.as_str(), "Beta", "the highlight clamped to the surviving last row");
}

/// Ride the Map on Alpha, then open the swap prompt for Gamma.
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
    app.apply_gesture(Gesture::Step(2)); // → the Routes row
    app.apply_gesture(Gesture::Press); // the sheet → Route menu
    app.apply_gesture(Gesture::Step(2)); // highlight Gamma
    app.apply_gesture(Gesture::Press); // a different route mid-ride → the swap prompt
    assert!(matches!(app.top_screen(), Screen::RouteSwap(_)), "the swap prompt is up");
    app
}

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

#[test]
fn apply_pushes_pops_replaces_and_returns_home() {
    let mut stack: Stack = Stack::new();
    let _ = stack.push(Screen::Home(HomeScreen::new()));
    let _ = stack.push(Screen::Map(MapScreen::new()));

    apply(&mut stack, Transition::Push(Screen::Menu(MenuScreen::new())));
    assert_eq!(stack.len(), 3);
    assert!(matches!(stack.last(), Some(Screen::Menu(_))));
    apply(&mut stack, Transition::Pop);
    assert!(matches!(stack.last(), Some(Screen::Map(_))), "Pop returns to caller");

    apply(&mut stack, Transition::Replace(Screen::Menu(MenuScreen::new())));
    assert_eq!(stack.len(), 2);
    assert!(matches!(stack.last(), Some(Screen::Menu(_))));

    apply(&mut stack, Transition::Home);
    assert_eq!(stack.len(), 1);
    assert!(matches!(stack.last(), Some(Screen::Home(_))));

    apply(&mut stack, Transition::Pop);
    assert_eq!(stack.len(), 1, "the Home root is never popped");
}

#[test]
fn pausing_swaps_the_map_for_the_paused_page() {
    let bytes = build_min_obcm(0xF800);
    let mut app = App::new(AppState::new(0, 0, 0.05));
    app.test_mount_store();
    app.test_start_ride(); // a tracking ride, so the map's press pauses (not the browse-map start card)

    // Sample the sea backdrop clear of the map chrome: the clock digits end ~y28, the bottom-centre
    // "No GPS Fix" chip band starts ~y74 on the 120px test frame, and the scale bar owns the
    // bottom-left, so mid-right between them is bare map.
    let map = render_120(&mut app, &bytes);
    let backdrop = map.get(95, 45);

    let mut press = keys(&[
        InputEvent::Button(ButtonEvent::Down(Button::Select)),
        InputEvent::Button(ButtonEvent::Up(Button::Select)),
    ]);
    app.handle_input(InputClock(0), &mut press);
    assert_eq!(app.mode(), Mode::Paused, "press paused the ride");

    let paused = render_120(&mut app, &bytes);
    let page = paused.get(95, 45);
    assert_ne!(page, backdrop, "pausing replaced the view");
    assert!(page.r() > backdrop.r(), "the parchment page is lighter than the sea backdrop");
}

// Inspect/Pan mode: a Map sub-mode driven by the shared `AppState::pan`.

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

#[test]
fn pan_freezes_camera_against_fixes() {
    let (mut st, _act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    st.enter_pan(false, 0);
    st.update(&mut ReplayFix(Some(Fix::at(5000, 7000))));
    assert_eq!((st.cam_lon, st.cam_lat), (0, 0), "the frozen camera ignores the fix");
    assert_eq!(st.user_fix.map(|f| (f.lon, f.lat)), Some((7000, 5000)), "but the fix is recorded");
}

/// Inspect snapshots the live heading once, so later GPS courses cannot rotate a detached map.
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

/// Without a route the ring is its two remaining stations; Select-hold alternates the Free axes.
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

/// Route movement moves the distance cursor rather than a compass axis, and clamps at both ends.
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

/// Pick the bike type on the create-route sheet, then "reboot": encode → decode → a fresh App
/// adopts the blob. A drawer is not a settings subtree, so the commit arms the save on its own pass.
#[test]
fn bike_type_is_picked_from_the_route_plan_sheet_and_persists_across_reboot() {
    use crate::settings::BikeType;
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    app.test_mount_store();

    // The confirm card the rider reaches from a POI detail, seeded directly: the browse that gets
    // there needs a fix, a corridor snapshot and a queried map, none of which this test is about.
    app.ui.stack.truncate(1); // [Home]
    let _ = app.ui.stack.push(crate::harness::support::selected_place());

    // A page slide owns the sheet's input while it runs, so each gesture waits for the last slide
    // to land, on a clock that only moves forward.
    let mut ms = 0;
    let owes_a_save = |app: &mut App, ms: u32| quiet_pass(app, ms).effects.settings.take().is_some();

    assert!(app.apply_chord(crate::input::Chord::Context), "the confirm card declares a context");
    assert!(matches!(app.top_screen(), crate::Screen::ContextDrawer(_)), "the sheet, over the card");
    assert!(!owes_a_save(&mut app, ms), "opening a sheet changes no setting");

    // Row 0 is the only row: press into its editor, browse two types on, commit.
    app.apply_gesture(Gesture::Press);
    ms += 400; // let the page slide land
    app.advance_animations(obc_ports::InputClock(ms));
    app.apply_gesture(Gesture::Step(2)); // Road → Gravel → MTB
    assert_eq!(app.settings().bike_type, BikeType::Road, "browsing commits nothing");
    app.apply_gesture(Gesture::Press);
    ms += 400;
    app.advance_animations(obc_ports::InputClock(ms));
    assert_eq!(app.settings().bike_type, BikeType::Mtb, "Select wrote the type the editor was on");
    assert!(owes_a_save(&mut app, ms), "a drawer is not a settings subtree: the save is armed now");

    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), crate::Screen::PoiDetail(_)), "the card is still under it");

    // Simulated reboot: the persisted blob seeds a fresh App (the boot path of both hosts).
    let blob = crate::settings::encode(app.settings());
    let restored = crate::settings::decode(&blob).expect("clean blob decodes");
    let mut app2 = App::new_idle(AppState::new(0, 0, 0.05));
    app2.test_mount_store();
    app2.set_settings(restored);
    assert_eq!(app2.settings().bike_type, BikeType::Mtb, "the bike type survives the reboot");
}

/// The map sheet is the only way to all three display switches, and their answers survive a
/// reboot. A drawer is not a settings subtree, so each flip arms a save on the pass it happened on.
#[test]
fn the_map_sheet_reaches_all_three_settings_and_survives_a_reboot() {
    let mut app = App::new(AppState::new(0, 0, 0.05)); // [Home, Map]
    app.test_mount_store();
    let s = app.settings();
    assert!(s.map_clock && s.map_scale_bar && s.map_contours, "all three default on");

    // The chord over the riding Map, then the sheet's last row — the one only the Map declares.
    assert!(app.apply_chord(crate::input::Chord::Context), "the riding Map declares a context");
    app.apply_gesture(Gesture::Step(-1)); // wrap to the fifth row, Map display
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), crate::Screen::ContextDrawer(_)), "a sheet, not a screen");

    app.apply_gesture(Gesture::Press);
    assert!(!app.settings().map_clock, "row 0 flipped the clock");
    assert!(settings_dirty(&mut app), "a drawer is not a settings subtree: the save is armed now");

    app.apply_gesture(Gesture::Step(1));
    app.apply_gesture(Gesture::Press);
    assert!(!app.settings().map_scale_bar, "row 1 flipped the scale bar");
    assert!(settings_dirty(&mut app));

    app.apply_gesture(Gesture::Step(1));
    app.apply_gesture(Gesture::Press);
    assert!(!app.settings().map_contours, "row 2 flipped the contours");
    assert!(settings_dirty(&mut app));

    assert!(matches!(app.top_screen(), crate::Screen::ContextDrawer(_)));
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), crate::Screen::Map(_)), "Back lands on the Map, not on the sheet above");

    // Simulated reboot: the persisted blob seeds a fresh App (the boot path of both hosts).
    let blob = crate::settings::encode(app.settings());
    let restored = crate::settings::decode(&blob).expect("clean blob decodes");
    let mut app2 = App::new_idle(AppState::new(0, 0, 0.05));
    app2.test_mount_store();
    app2.set_settings(restored);
    let s = app2.settings();
    assert!(!s.map_clock && !s.map_scale_bar && !s.map_contours, "all three choices survive the reboot");
}

/// Back-hold always reaches the main menu, from one representative of every screen family.
#[test]
fn the_global_escape_reaches_the_menu_from_every_family() {
    /// One family: its name, and how to reach it from `[Home, Map]`.
    type Family = (&'static str, fn(&mut App));
    let families: [Family; 7] = [
        ("the Home root", |app| apply(&mut app.ui.stack, Transition::Home)),
        ("a riding view", |app| apply(&mut app.ui.stack, Transition::Root(Screen::Map(MapScreen::new())))),
        ("the paused page", |app| apply(&mut app.ui.stack, Transition::Push(Screen::RideControl(RideControl::new())))),
        ("a settings page", |app| {
            apply(&mut app.ui.stack, Transition::Push(Screen::Settings(crate::screen::SettingsPage::hub())));
            apply(
                &mut app.ui.stack,
                Transition::Push(Screen::Display(crate::screen::SettingsPage::new(
                    &crate::screen::settings::page::DISPLAY,
                ))),
            );
        }),
        ("a nav list", |app| apply(&mut app.ui.stack, Transition::Push(Screen::RouteMenu(RouteMenuScreen::new())))),
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

#[test]
fn the_escape_takes_the_context_sheet_with_it() {
    let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
    assert!(app.apply_chord(crate::input::Chord::Context));
    app.apply_gesture(Gesture::BackHold);
    assert!(matches!(app.top_screen(), Screen::Menu(_)));
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::Map(_)), "Back out of the Menu lands on the base, not the sheet");
}

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

/// A blocking modal refuses both the chord and the escape. The recovered-ride card refuses only
/// the escape: leaving it would strand a recovered recording that Back cannot dismiss either.
#[test]
fn a_card_the_rider_must_answer_refuses_the_escape() {
    let mut app = App::new(AppState::new(0, 0, 1.0));
    app.set_ble_status(crate::BleStatus { link: crate::BleLink::Connected, paired: false, passkey: Some(123_456) });
    assert!(matches!(app.top_screen(), Screen::Passkey(_)), "the passkey card is up");
    app.apply_gesture(Gesture::BackHold);
    assert!(matches!(app.top_screen(), Screen::Passkey(_)), "a blocking modal refuses the escape");

    // The refusal is asked of the base screen, not of whatever sheet is on top.
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    assert!(app.offer_damaged_ride(crate::RideDamage::Payload), "the recovery card is offered");
    assert!(app.apply_chord(crate::input::Chord::Quick), "…and a quick sheet over it is harmless");
    app.apply_gesture(Gesture::BackHold);
    assert!(
        matches!(app.top_screen(), Screen::QuickDrawer(_)),
        "a sheet the rider opened over the card is not consent to walk away from it"
    );

    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    assert!(app.offer_damaged_ride(crate::RideDamage::Payload));
    app.apply_gesture(Gesture::BackHold);
    assert!(matches!(app.top_screen(), Screen::RideRecovery(_)), "the recovery decision stays put");
}

/// The escape rewinds to the Menu instead of pushing a new one, so repeated laps of escape and
/// re-descent cannot walk the stack to `MAX_DEPTH`, where the next host-pushed card is dropped.
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
        // From lap 1 the dial is still on Settings: the escape rewinds to the same Menu, with the
        // station the rider last used selected.
        app.apply_gesture(Gesture::Press); // → the Settings list (Ride first)
        assert!(matches!(app.top_screen(), Screen::Settings(_)), "lap {lap}: the Menu kept its station");
        app.apply_gesture(Gesture::Press); // → Ride (Data fields is the first row)
        app.apply_gesture(Gesture::Press); // → Fields
        assert!(matches!(app.top_screen(), Screen::StatFields(_)), "lap {lap} reached the fields editor");
        deepest = deepest.max(app.ui.stack.len());

        app.apply_gesture(Gesture::BackHold);
        assert!(matches!(app.top_screen(), Screen::Menu(_)), "lap {lap} escaped to the Menu");
        let settled = app.ui.stack.len();
        app.apply_gesture(Gesture::BackHold);
        assert_eq!(app.ui.stack.len(), settled, "lap {lap}: a second escape moved the stack");
        assert_eq!(settled, 3, "lap {lap}: the escape rewound to the Menu the rider already had");
    }
    assert!(deepest < crate::screen::MAX_DEPTH, "the reachable depth is {deepest}, at the ceiling of MAX_DEPTH");

    app.on_alerts(Alert::RecordingFailed);
    assert!(matches!(app.top_screen(), Screen::Warning(_)), "the host warning must still fit over the escape");
}

/// The quick drawer's Settings row starts central settings from the root pair and drops the
/// descent the squeeze came from, so laps of squeeze and press stay at one depth and Back leaves
/// settings for the view the rider rides on.
#[test]
fn laps_of_the_drawer_settings_row_stay_on_the_root() {
    /// One lap: the squeeze, the steps to the settings control, and the press.
    fn lap(app: &mut App, ms: &mut u32) {
        assert!(app.apply_chord(crate::input::Chord::Quick), "the squeeze opens the sheet");
        *ms += 1_000;
        app.advance_animations(InputClock(*ms)); // settle the open slide
        for _ in 0..2 {
            app.apply_gesture(Gesture::Step(1)); // brightness → bluetooth → settings
        }
        app.apply_gesture(Gesture::Press);
    }

    fn shape(app: &App) -> Vec<&'static str> {
        app.ui.stack.iter().map(Screen::name).collect()
    }

    let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map], riding
    app.set_backlight_available(true);
    let mut ms = 1_000;
    for lap_no in 0..8 {
        lap(&mut app, &mut ms);
        assert_eq!(shape(&app), ["Home", "Map", "Settings"], "lap {lap_no} left the root pair");
        // Two pages down the settings tree, which the next lap has to drop.
        app.apply_gesture(Gesture::Step(4)); // → System
        app.apply_gesture(Gesture::Press);
        app.apply_gesture(Gesture::Step(1)); // → Date & time, a page rather than an editor sheet
        app.apply_gesture(Gesture::Press);
        assert_eq!(shape(&app), ["Home", "Map", "Settings", "System", "DateTime"], "lap {lap_no} did not descend");
    }

    lap(&mut app, &mut ms);
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::Map(_)), "Back out of settings leaves for the riding view");

    app.on_alerts(Alert::RecordingFailed);
    assert!(matches!(app.top_screen(), Screen::Warning(_)), "the host card still has room after the laps");

    // On the idle screensaver there is no view under the root, so settings is itself what the
    // pair keeps: the lap lands on the settings the rider left, and Back reaches Home.
    let mut idle = App::new_idle(AppState::new(0, 0, 1.0)); // [Home]
    idle.set_backlight_available(true);
    let mut ms = 1_000;
    for lap_no in 0..4 {
        lap(&mut idle, &mut ms);
        assert_eq!(shape(&idle), ["Home", "Settings"], "idle lap {lap_no} stacked a second settings");
        if lap_no == 0 {
            idle.apply_gesture(Gesture::Step(4)); // → System
        }
        // From lap 1 the row is still where the rider left it, because the lap lands on the
        // settings screen they had rather than on a fresh one.
        idle.apply_gesture(Gesture::Press);
        assert_eq!(shape(&idle), ["Home", "Settings", "System"], "idle lap {lap_no} did not descend");
    }
    lap(&mut idle, &mut ms);
    idle.apply_gesture(Gesture::Back);
    assert!(matches!(idle.top_screen(), Screen::Home(_)), "Back out of settings reaches the screensaver");
}

/// The Assistant chord answers from the root pair too, so the rider reaches the same place from
/// any depth and Back leaves it for the view they ride on. The deepest page is the hard case: the
/// chord used to spend a slot on it, and at the ceiling it had to fall back to the bare Home root.
#[test]
fn the_assistant_chord_lands_on_the_root_pair_from_a_deep_page() {
    use crate::screen::MAX_DEPTH;

    fn shape(app: &App) -> Vec<&'static str> {
        app.ui.stack.iter().map(Screen::name).collect()
    }

    let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map], riding
    while app.ui.stack.len() < MAX_DEPTH {
        let _ = app.ui.stack.push(Screen::Menu(MenuScreen::new()));
    }

    assert!(app.apply_chord(crate::input::Chord::Assistant), "the hold opens the Assistant");
    assert_eq!(shape(&app), ["Home", "Map", "Assistant"], "the descent the chord came from went with it");

    // A question is a door, so the room for what it opens is part of the landing.
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::FindPlace(_)), "the first question still opens");
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::Assistant(_)));
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::Map(_)), "Back leaves the Assistant for the riding view");

    // On the idle Home the Assistant is itself the screen the root pair keeps, so a repeat lands
    // on the one the rider left rather than stacking a second copy over it.
    let mut idle = App::new_idle(AppState::new(0, 0, 1.0)); // [Home]
    assert!(idle.apply_chord(crate::input::Chord::Quick), "the squeeze opens a sheet over the screensaver");
    assert!(idle.apply_chord(crate::input::Chord::Assistant));
    assert_eq!(shape(&idle), ["Home", "Assistant"], "the sheet went with the descent");
    idle.apply_gesture(Gesture::Step(1)); // move the question cursor off the first row
    assert!(idle.apply_chord(crate::input::Chord::Assistant));
    assert_eq!(shape(&idle), ["Home", "Assistant"], "the repeat stayed put");
    assert!(
        matches!(idle.top_screen(), Screen::Assistant(s) if s.selected == 1),
        "…on the question the rider was reading, not on a fresh page"
    );
}

/// A sheet needs a slot of its own, so at the ceiling the squeeze is refused and reports that
/// nothing moved. The alternative is `apply` dropping the push behind a `debug_assert`: a rider
/// squeezing at a full stack and getting silence.
#[test]
fn a_squeeze_at_the_ceiling_is_refused() {
    use crate::screen::MAX_DEPTH;

    let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map], riding
    app.set_backlight_available(true);
    while app.ui.stack.len() < MAX_DEPTH - 1 {
        let _ = app.ui.stack.push(Screen::Menu(MenuScreen::new()));
    }
    assert!(app.apply_chord(crate::input::Chord::Quick), "with the last slot free the sheet opens");
    assert!(matches!(app.top_screen(), Screen::QuickDrawer(_)));
    assert!(app.apply_chord(crate::input::Chord::Quick), "and the same chord closes it again");

    let _ = app.ui.stack.push(Screen::Menu(MenuScreen::new()));
    assert_eq!(app.ui.stack.len(), MAX_DEPTH, "the stack is at the ceiling");
    assert!(!app.apply_chord(crate::input::Chord::Quick), "a full stack refuses the squeeze");
    assert_eq!(app.ui.stack.len(), MAX_DEPTH, "and nothing moved");
}

/// The deepest screen stack a rider reaches, walked instead of asserted from one hand-written
/// path. The alphabet is the three navigating gestures — a step, a press and a guarded hold — and
/// the three device-wide chords, so this bounds every way down, including the ways a sheet opens.
///
/// `MAX_DEPTH` is not a wall: `apply` drops an overflowing push behind a `debug_assert`, and a
/// release build then loses the screen without a sound. So the reserve the ceiling leaves is
/// pinned rather than assumed. Both doors that open over a descent — the drawer's settings row
/// and the Assistant chord — land on the root pair, so no way down composes onto another one, and
/// the walk stops short of the ceiling. A card is measured against the deepest stack with no
/// sheet on it, because `card_scheduler::land` takes any open drawer off before the card goes on.
#[test]
fn the_deepest_descent_stops_short_of_max_depth() {
    use crate::input::Chord;

    /// Steps taken on a page before its gesture. The probe below proves one more step reaches no
    /// page the walk misses, so a list that outgrows this cannot go under-walked in silence.
    const ROWS: i32 = 16;
    /// A tree that outgrows this is no longer the one these numbers describe.
    const VISITS: usize = 500;
    /// The deepest stack the walk reaches: seven pages with one sheet over them.
    const DEEPEST: usize = 8;
    /// The deepest stack a host card lands on — that descent with the sheet taken off.
    const DEEPEST_FOR_A_CARD: usize = 7;

    /// One move: `k` steps then that gesture, or one device-wide chord.
    #[derive(Clone, Copy, Debug)]
    enum Move {
        Press(i32),
        Hold(i32),
        Chord(Chord),
    }

    fn seeded() -> App {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map], riding
        app.test_mount_store();
        app.set_backlight_available(true);
        app.set_routes_with_ids(&test_routes(), &IDS3);
        app.set_rides(
            &[
                crate::RideEntry { id: 7, summary: ride_summary("Ride A") },
                crate::RideEntry { id: 9, summary: ride_summary("Ride B") },
            ],
            &[],
        );
        // The escape is the way in from a riding view, and [Home, Map, Menu] is the deepest root a
        // descent starts from. On the Map itself a step zooms and a press pauses.
        app.apply_gesture(Gesture::BackHold);
        app
    }

    /// One path replayed from a fresh device, a frame of animation after each move so a page that
    /// slides has settled before its rows answer.
    fn walk(path: &[Move]) -> App {
        let mut app = seeded();
        let mut ms = 1_000;
        for mv in path {
            let (steps, gesture) = match *mv {
                Move::Press(k) => (k, Gesture::Press),
                Move::Hold(k) => (k, Gesture::Hold),
                Move::Chord(chord) => {
                    app.apply_chord(chord);
                    ms += 1_000;
                    app.advance_animations(InputClock(ms));
                    continue;
                }
            };
            for _ in 0..steps {
                app.apply_gesture(Gesture::Step(1));
            }
            app.apply_gesture(gesture);
            ms += 1_000;
            app.advance_animations(InputClock(ms));
        }
        app
    }

    fn shape(app: &App) -> Vec<&'static str> {
        app.ui.stack.iter().map(Screen::name).collect()
    }

    /// The slots a host card would find: the stack under any open sheet.
    fn under_the_sheet(app: &App) -> usize {
        crate::screen::base_index(&app.ui.stack) + 1
    }

    // Each stack shape is expanded once, from the first path that reaches it: every row of that one
    // representative is then pressed and held.
    let mut seen: std::collections::BTreeSet<Vec<&'static str>> = std::collections::BTreeSet::new();
    let mut pending: Vec<(Vec<Move>, Vec<&'static str>, usize)> =
        vec![(Vec::new(), shape(&seeded()), under_the_sheet(&seeded()))];
    let mut deepest: Vec<&'static str> = Vec::new();
    let mut deepest_for_a_card = 0usize;
    let mut visits = 0usize;
    while let Some((path, here, base)) = pending.pop() {
        if !seen.insert(here.clone()) {
            continue;
        }
        visits += 1;
        assert!(visits < VISITS, "the walk outgrew its budget on {path:?}");
        deepest_for_a_card = deepest_for_a_card.max(base);
        if here.len() > deepest.len() {
            deepest = here;
        }
        for row in [Move::Press as fn(i32) -> Move, Move::Hold as fn(i32) -> Move] {
            let mut reached: std::collections::BTreeSet<Vec<&'static str>> = std::collections::BTreeSet::new();
            for k in 0..=ROWS {
                let mut next = path.clone();
                next.push(row(k));
                let reached_app = walk(&next);
                let child = shape(&reached_app);
                if k == ROWS {
                    assert!(reached.contains(&child), "ROWS is too small: {k} steps reach {child:?}");
                } else if reached.insert(child.clone()) {
                    pending.push((next, child, under_the_sheet(&reached_app)));
                }
            }
        }
        for chord in [Chord::Quick, Chord::Context, Chord::Assistant] {
            let mut next = path.clone();
            next.push(Move::Chord(chord));
            let reached_app = walk(&next);
            let child = shape(&reached_app);
            pending.push((next, child, under_the_sheet(&reached_app)));
        }
    }

    assert!(seen.len() >= 300, "the walk reached only {} stacks — it stopped early", seen.len());
    assert!(
        seen.contains(&vec!["Home", "Map", "Menu", "Settings", "Ride", "StatFields", "AddField"]),
        "the walk missed the documented deepest rider path"
    );
    assert_eq!(deepest.len(), DEEPEST, "the deepest stack is now {deepest:?}");
    assert_eq!(deepest_for_a_card, DEEPEST_FOR_A_CARD, "the deepest stack with no sheet on it moved");
    // The reserve, stated as the property it protects: every reachable stack has a free slot, so
    // no arrival is dropped behind the `debug_assert` and lost in a release build.
    assert!(
        deepest.len() < crate::screen::MAX_DEPTH,
        "a descent reaches the ceiling, where the next arrival is dropped without a sound"
    );
}

/// A shutdown in progress is not cancellable by either device-wide input. It cannot be expressed
/// in `Caps`: the frame is a page of the quick drawer, and a drawer must never declare
/// `blocks_chords`, because the chord that closes it would trip over that. Both owners check it at
/// run time instead.
#[test]
fn nothing_cancels_a_shutdown_already_in_progress() {
    /// A device on the terminal POWERING OFF frame, reached the way a rider reaches it.
    fn powering_off() -> App {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
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

    // The card sweep runs in the same `handle_input` that applied the completed hold, so a card
    // landing there would take the frame away. It is refused instead.
    let mut app = powering_off();
    app.on_alerts(Alert::RecordingFailed);
    assert!(app.power_off_requested(), "a host card must not cancel a shutdown in progress");
    assert!(matches!(app.top_screen(), Screen::QuickDrawer(_)), "the panel keeps the powering-off frame");
    // The remote DFU card arrives from neither the card scheduler nor a screen transition, so it
    // refuses this on its own.
    let mut app = powering_off();
    assert!(!app.open_remote_dfu_check(), "a phone's install request must not open over a shutdown");
    assert!(app.power_off_requested(), "…and must not cancel it");
    assert!(matches!(app.top_screen(), Screen::QuickDrawer(_)), "the panel keeps the powering-off frame");
}

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

/// A drawer is transient chrome: a host card arriving over an open sheet takes the sheet with it,
/// so dismissing the card lands the rider on the screen they were on.
#[test]
fn a_card_landing_over_a_sheet_takes_the_sheet_with_it() {
    for chord in [crate::input::Chord::Quick, crate::input::Chord::Context] {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
        assert!(app.apply_chord(chord));
        assert!(app.ui.stack.iter().any(|s| s.is_overlay()), "{chord:?} opened a sheet");

        app.on_alerts(Alert::RecordingFailed);
        assert!(matches!(app.top_screen(), Screen::Warning(_)), "the card landed");
        assert!(!app.ui.stack.iter().any(|s| s.is_overlay()), "{chord:?}: the sheet went with it");

        app.apply_gesture(Gesture::Press); // dismiss the card
        assert!(matches!(app.top_screen(), Screen::Map(_)), "{chord:?}: dismissing lands on the base");
    }
}
