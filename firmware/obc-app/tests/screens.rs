//! Screen-stack tests: navigation [`Transition`]s per gesture, the guarded-action "needs a completed
//! hold" rule, the stack discipline ([`apply`]), and a render snapshot proving pausing swaps the
//! map view for the full-screen Paused page.

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::RgbColor; // for `Rgb888::r()` in the compositing snapshot
use obc_app::activity::Activity;
use obc_app::screen::{
    apply, Ctx, HomeScreen, MapScreen, MenuScreen, PoiScratch, RideControl, RouteMenuScreen, RouteOverviewScreen,
    RouteSwapScreen, Screen, ScreenTick, Stack, Transition,
};
use obc_app::{
    App, AppState, Button, ButtonEvent, CameraMode, Fix, Gesture, InputClock, InputEvent, Mode, PanAxis, RideClock,
    RouteSummary, Sensors, Settings, TrackAction, MAX_ROUTES,
};
use obc_reader::{rgb565_to_rgb888, BBox, MapCache, MapTables, Reader, SliceSource};

mod common;
use common::{build_min_obcm, build_min_obcm_profiles, keys, Buf, NoFix, ReplayFix};

/// A throwaway default [`Settings`] satisfying [`Ctx`]'s `&mut` borrow. The non-settings screens
/// under test never touch it, so each call leaks a fresh (non-aliasing) block — fine in a short-lived
/// test process.
fn leaked_settings() -> &'static mut Settings {
    Box::leak(Box::new(Settings::default()))
}

/// An empty [`PoiScratch`] for the handle `Ctx` — leaked so it satisfies the `&'a` borrow without a
/// lifetime dance in each helper. The non-POI screens under test never read it.
fn leaked_scratch() -> &'static PoiScratch {
    Box::leak(Box::new(PoiScratch::new()))
}

/// An empty [`NavProfiles`](obc_app::NavProfiles) for the handle `Ctx` — leaked for the same `&'a`
/// reason as the scratch (a `&NavProfiles::EMPTY` const can't promote to `'static`). The screens
/// under test aren't the Bike-type screen, so they never read it.
fn leaked_profiles() -> &'static obc_app::NavProfiles {
    Box::leak(Box::new(obc_app::NavProfiles::new()))
}

/// A handle [`Ctx`] over freshly-made state/activity. The Route-menu tests pass a catalog via
/// [`route_ctx`].
fn ctx<'a>(state: &'a mut AppState, activity: &'a mut Activity) -> Ctx<'a> {
    Ctx {
        state,
        activity,
        settings: leaked_settings(),
        routes: &[],
        rides: &[],
        nav_profiles: leaked_profiles(),
        poi_scratch: leaked_scratch(),
        now_ms: 0,
    }
}

/// A handle [`Ctx`] carrying a route catalog, for the Route-menu tests.
fn route_ctx<'a>(state: &'a mut AppState, activity: &'a mut Activity, routes: &'a [RouteSummary]) -> Ctx<'a> {
    Ctx {
        state,
        activity,
        settings: leaked_settings(),
        routes,
        rides: &[],
        nav_profiles: leaked_profiles(),
        poi_scratch: leaked_scratch(),
        now_ms: 0,
    }
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
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    act.start_session(); // the riding map (tracking) — press pauses; a browse map would open the start card
    let t = MapScreen::new().handle(Gesture::Press, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::Push(Screen::RideControl(_))));
    assert_eq!(act.mode, Mode::Paused, "pausing stops tracking immediately");
}

#[test]
fn map_turn_zooms_in_place() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    let z0 = st.zoom;
    let t = MapScreen::new().handle(Gesture::Turn(2), &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::None));
    assert!(st.zoom > z0, "clockwise turn zooms in");
}

/// Map zoom is `×ZOOM_STEP` per detent, compounding — pins the per-detent multiply so a regression
/// to an additive step is caught.
#[test]
fn map_turn_multiplies_zoom_per_detent() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    MapScreen::new().handle(Gesture::Turn(1), &mut ctx(&mut st, &mut act));
    let one = st.zoom;
    assert!(one > 1.0, "one detent zooms in past 1.0, got {one}");
    MapScreen::new().handle(Gesture::Turn(1), &mut ctx(&mut st, &mut act));
    // The second detent multiplies again: zoom/one == one/1.0 (a constant ratio per detent).
    assert!((st.zoom / one - one).abs() < 1e-3, "each detent is the same ×ratio, got {} then {}", one, st.zoom);
}

/// A huge forward turn saturates at `MAX_ZOOM` instead of overflowing to `inf` (a `Turn(1000)` would
/// multiply `1.2^1000` straight to infinity).
#[test]
fn map_turn_saturates_at_max_zoom() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    MapScreen::new().handle(Gesture::Turn(1000), &mut ctx(&mut st, &mut act));
    let saturated = st.zoom;
    assert!(saturated.is_finite(), "a huge turn must clamp, not overflow to inf, got {saturated}");
    // A second huge turn can't push it any higher — it's pinned at the cap.
    MapScreen::new().handle(Gesture::Turn(1000), &mut ctx(&mut st, &mut act));
    assert_eq!(st.zoom, saturated, "already at MAX_ZOOM — further zoom-in is a no-op");
}

/// A huge backward turn saturates at `MIN_ZOOM` instead of underflowing toward 0 (which would invert
/// / blank the view).
#[test]
fn map_turn_saturates_at_min_zoom() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    MapScreen::new().handle(Gesture::Turn(-1000), &mut ctx(&mut st, &mut act));
    let saturated = st.zoom;
    assert!(saturated > 0.0, "min-zoom clamp keeps the scale positive, got {saturated}");
    MapScreen::new().handle(Gesture::Turn(-1000), &mut ctx(&mut st, &mut act));
    assert_eq!(st.zoom, saturated, "already at MIN_ZOOM — further zoom-out is a no-op");
}

#[test]
fn map_back_hold_opens_the_menu() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    let t = MapScreen::new().handle(Gesture::BackHold, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::Push(Screen::Menu(_))));
}

#[test]
fn ride_control_resume_is_a_press_that_pops() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Paused));
    let mut rc = RideControl::new(); // starts on Resume
    assert!(!rc.selection_is_guarded());
    let t = rc.handle(Gesture::Press, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::Pop), "Resume returns to the caller");
    assert_eq!(act.mode, Mode::Riding);
}

#[test]
fn ride_control_back_resumes() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Paused));
    let t = RideControl::new().handle(Gesture::Back, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::Pop));
    assert_eq!(act.mode, Mode::Riding, "back cancels the pause");
}

#[test]
fn guarded_action_needs_a_completed_hold_not_a_press() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Paused));
    let mut rc = RideControl::new();
    rc.handle(Gesture::Turn(1), &mut ctx(&mut st, &mut act)); // move to Finish (guarded)
    assert!(rc.selection_is_guarded());

    // A press must NOT commit an irreversible action.
    let t = rc.handle(Gesture::Press, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::None));
    assert_eq!(act.mode, Mode::Paused, "a stray press can't finish the ride");

    // A completed hold (the recognizer only emits `Hold` once the threshold is
    // crossed) is what confirms it.
    let t = rc.handle(Gesture::Hold, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::Home), "Finish clears back to Home");
    assert_eq!(act.mode, Mode::Idle);
}

#[test]
fn hold_on_a_non_guarded_item_does_nothing() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Paused));
    let t = RideControl::new().handle(Gesture::Hold, &mut ctx(&mut st, &mut act)); // on Resume
    assert!(matches!(t, Transition::None));
    assert_eq!(act.mode, Mode::Paused);
}

#[test]
fn menu_back_returns_to_caller() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    let t = MenuScreen::new().handle(Gesture::Back, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::Pop));
}

/// The Menu's compass-needle sweep contract: a turn arms a per-frame wake, the sweep converges in
/// well under a second of ticks, and a settled menu is [`ScreenTick::idle`] — so a resting menu
/// costs the event-driven host no timed repaints (the invariant
/// `ms_until_next_wake_reports_the_home_minute_then_none_on_a_static_menu` also leans on).
#[test]
fn menu_needle_sweep_arms_then_settles() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    let mut m = MenuScreen::new();
    assert_eq!(m.tick_timers(0), ScreenTick::idle(), "a fresh menu has no animation pending");

    m.handle(Gesture::Turn(1), &mut ctx(&mut st, &mut act));
    let t0 = m.tick_timers(1_000);
    assert!(t0.next_wake_ms.is_some(), "a turn puts the sweep in flight");

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
fn home_press_and_back_hold_both_open_the_menu() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    // Both the press and the back-hold now open the compass Menu — the single door into the app.
    let p = HomeScreen::new().handle(Gesture::Press, &mut ctx(&mut st, &mut act));
    assert!(matches!(p, Transition::Push(Screen::Menu(_))), "press opens the Menu");
    let b = HomeScreen::new().handle(Gesture::BackHold, &mut ctx(&mut st, &mut act));
    assert!(matches!(b, Transition::Push(Screen::Menu(_))), "back-hold opens the Menu too");
    // A turn on Home is ignored.
    let t = HomeScreen::new().handle(Gesture::Turn(1), &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::None), "encoder turns on Home are ignored");
}

#[test]
fn menu_routes_station_opens_the_route_menu() {
    // The Route menu is reached from the Menu's Routes station (selected 0 by default).
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    let t = MenuScreen::new().handle(Gesture::Press, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::Push(Screen::RouteMenu(_))), "Menu → Routes → Route menu");
}

#[test]
fn route_menu_press_opens_the_overview_and_preloads_the_route() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    let routes = test_routes();
    let mut rm = RouteMenuScreen::new();
    rm.handle(Gesture::Turn(1), &mut route_ctx(&mut st, &mut act, &routes)); // highlight route 1
    let t = rm.handle(Gesture::Press, &mut route_ctx(&mut st, &mut act, &routes));
    assert!(matches!(t, Transition::Push(Screen::RouteOverview(_))), "picking opens the overview");
    assert_eq!(act.active_route, Some(1), "the pick preloads the route so the overview gets a profile");
    assert_eq!(act.mode, Mode::Idle, "no riding yet — the overview's START does that");
    assert!(!act.is_tracking(), "and no session either");
}

#[test]
fn overview_start_begins_the_session_and_opens_the_map() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    st.mode = CameraMode::Free; // the map-viewer default; starting must flip to Follow
    st.heading_up = false;
    act.active_route = Some(1); // the Route menu preloaded the preview
    let routes = test_routes();
    let t = RouteOverviewScreen::new(1, None).handle(Gesture::Press, &mut route_ctx(&mut st, &mut act, &routes));
    assert!(matches!(t, Transition::Root(Screen::Map(_))), "starting lands on a clean [Home, Map]");
    assert_eq!(act.mode, Mode::Riding, "START begins tracking");
    assert_eq!(act.active_route, Some(1), "the previewed route is the active one");
    assert!(act.is_tracking(), "START opens a tracking session");
    // Starting drops into the riding view: follow + heading-up, seeded at the start.
    assert_eq!(st.mode, CameraMode::Follow);
    assert!(st.heading_up);
    assert_eq!((st.cam_lon, st.cam_lat), (100, 100), "camera seeded at the route start");
    assert!(st.zoom > 0.2 && st.zoom < 0.25, "~0.5 m/px riding zoom, got {}", st.zoom);
}

#[test]
fn overview_back_cancels_and_restores_the_previous_route() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    act.active_route = Some(2); // the Route menu preloaded the preview…
    let routes = test_routes();
    // …over a previously loaded route 0, which back must put back.
    let t = RouteOverviewScreen::new(2, Some(0)).handle(Gesture::Back, &mut route_ctx(&mut st, &mut act, &routes));
    assert!(matches!(t, Transition::Pop), "back returns to the Route menu");
    assert_eq!(act.active_route, Some(0), "the previous route is restored");
    assert!(!act.is_tracking(), "browsing started nothing");
}

#[test]
fn route_menu_back_returns_to_caller() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    let t = RouteMenuScreen::new().handle(Gesture::Back, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::Pop));
}

#[test]
fn route_menu_with_no_routes_ignores_press() {
    // An empty catalog: press/turn are no-ops, so a routeless device can't "load" one.
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    let t = RouteMenuScreen::new().handle(Gesture::Press, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::None));
    assert_eq!(act.active_route, None);
}

// Loading a route mid-session: the swap / save prompt, Finish / Discard.

/// An activity that is already tracking route `r` (a session is open).
fn tracking(r: usize) -> Activity {
    let mut act = Activity::new(Mode::Riding);
    act.active_route = Some(r);
    act.start_session();
    act
}

#[test]
fn loading_a_different_route_mid_session_prompts() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), tracking(0));
    let routes = test_routes();
    let mut rm = RouteMenuScreen::new();
    rm.handle(Gesture::Turn(1), &mut route_ctx(&mut st, &mut act, &routes)); // highlight route 1
    let t = rm.handle(Gesture::Press, &mut route_ctx(&mut st, &mut act, &routes));
    assert!(matches!(t, Transition::Push(Screen::RouteSwap(_))), "a different route mid-ride asks");
    assert_eq!(act.active_route, Some(0), "the prompt hasn't changed the route yet");
}

#[test]
fn reselecting_the_active_route_mid_session_returns_to_the_map() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), tracking(1));
    let routes = test_routes();
    let mut rm = RouteMenuScreen::new();
    rm.handle(Gesture::Turn(1), &mut route_ctx(&mut st, &mut act, &routes)); // highlight the active route 1
    let t = rm.handle(Gesture::Press, &mut route_ctx(&mut st, &mut act, &routes));
    assert!(matches!(t, Transition::Root(Screen::Map(_))), "re-picking the active route just rides it");
}

#[test]
fn route_swap_swap_only_keeps_the_session() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), tracking(0));
    let before = act.session;
    let routes = test_routes();
    // Default selection (0) is "Swap route".
    let t = RouteSwapScreen::new(2).handle(Gesture::Press, &mut route_ctx(&mut st, &mut act, &routes));
    assert!(matches!(t, Transition::Root(Screen::Map(_))));
    assert_eq!(act.active_route, Some(2), "navigation swapped to the picked route");
    assert_eq!(act.session, before, "the tracking session continues unchanged");
    assert!(act.take_track_action().is_none(), "swap-only saves nothing");
}

#[test]
fn route_swap_save_and_new_saves_then_starts_a_fresh_session() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), tracking(0));
    let before = act.session;
    let routes = test_routes();
    let mut rs = RouteSwapScreen::new(2);
    rs.handle(Gesture::Turn(1), &mut route_ctx(&mut st, &mut act, &routes)); // highlight "Save & new"
    assert!(rs.selection_is_guarded());
    // A press must not commit the guarded option — only a completed hold.
    let t = rs.handle(Gesture::Press, &mut route_ctx(&mut st, &mut act, &routes));
    assert!(matches!(t, Transition::None), "a press can't confirm Save & new");
    assert!(act.take_track_action().is_none());

    let t = rs.handle(Gesture::Hold, &mut route_ctx(&mut st, &mut act, &routes));
    assert!(matches!(t, Transition::Root(Screen::Map(_))));
    assert_eq!(act.active_route, Some(2));
    assert_ne!(act.session, before, "a fresh session id");
    assert!(act.is_tracking());
    assert_eq!(act.take_track_action(), Some(TrackAction::Save), "the old ride is saved");
}

#[test]
fn ride_control_finish_saves_and_discard_discards() {
    // Finish (row 1) → save the ride.
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), tracking(0));
    act.mode = Mode::Paused;
    let mut rc = RideControl::new();
    rc.handle(Gesture::Turn(1), &mut ctx(&mut st, &mut act)); // → Finish
    let t = rc.handle(Gesture::Hold, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::Home));
    assert_eq!(act.mode, Mode::Idle);
    assert_eq!(act.active_route, None);
    assert!(!act.is_tracking(), "Finish ends the session");
    assert_eq!(act.take_track_action(), Some(TrackAction::Save));

    // Discard (row 2) → throw the ride away.
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), tracking(0));
    act.mode = Mode::Paused;
    let mut rc = RideControl::new();
    rc.handle(Gesture::Turn(2), &mut ctx(&mut st, &mut act)); // → Discard
    let t = rc.handle(Gesture::Hold, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::Home));
    assert!(!act.is_tracking());
    assert_eq!(act.take_track_action(), Some(TrackAction::Discard));
}

#[test]
fn list_window_keeps_the_selection_visible() {
    use obc_app::screen::window_start;
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
    app.set_routes(&test_routes());
    assert_eq!(app.mode(), Mode::Idle);
    press(&mut app); // Home → Menu (Routes selected)
    assert_eq!(app.mode(), Mode::Idle, "opening the menu doesn't start riding yet");
    press(&mut app); // Menu → Route menu
    assert_eq!(app.mode(), Mode::Idle, "opening the route list doesn't start riding yet");
    press(&mut app); // Route menu → Route overview (route preloads, still not riding)
    assert_eq!(app.mode(), Mode::Idle, "the overview previews; START is what rides");
    assert_eq!(app.activity.active_route, Some(0), "the preview loads the route");
    press(&mut app); // START RIDE → Map
    assert_eq!(app.mode(), Mode::Riding);
    assert_eq!(app.activity.active_route, Some(0));
}

// Route catalog capacity: `set_routes` truncates a host store larger than the resident catalog
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
fn set_routes_truncates_at_max_routes() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.set_routes(&many_routes(MAX_ROUTES + 50)); // 114 routes — well over the cap
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
fn set_routes_keeps_exactly_max_routes() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.set_routes(&many_routes(MAX_ROUTES));
    assert_eq!(app.routes().len(), MAX_ROUTES, "a card with exactly 64 routes loses none");
}

/// `set_routes` replaces, not appends: a rescan of a now-emptied card leaves an empty catalog, not
/// the stale entries.
#[test]
fn set_routes_replaces_the_previous_catalog() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.set_routes(&many_routes(10));
    assert_eq!(app.routes().len(), 10);
    app.set_routes(&[]); // card removed / emptied
    assert!(app.routes().is_empty(), "a rescan replaces the catalog rather than appending");
}

// ==================== live catalog: identity remap across rescans (#450) ====================
//
// The catalog carries durable object ids; every held catalog index — `active_route`, an open
// Route-menu highlight, a pending swap — is remapped by id on every `set_routes_with_ids`. These
// pin the sharpest latent bug in epic #447: a rescan that inserts/removes a route must never
// silently shift which route is navigated.

/// Ids for [`test_routes`] — deliberately non-positional, so an index-as-id shortcut can't pass.
const IDS3: [u16; 3] = [10, 20, 30];

/// The DoD case: while navigating route X, a *different* route is uploaded/deleted → the app
/// still navigates X, at its new index.
#[test]
fn rescan_keeps_active_route_on_the_same_route() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    let routes = test_routes(); // Alpha, Beta, Gamma
    app.set_routes_with_ids(&routes, &IDS3);
    app.activity.active_route = Some(1); // navigating Beta (id 20)

    // Delete Alpha: the list shrinks, Beta shifts 1 → 0 — navigation follows the identity.
    app.set_routes_with_ids(&routes[1..], &IDS3[1..]);
    assert_eq!(app.activity.active_route, Some(0), "shrunk list: the index moved with the route");
    assert_eq!(app.routes()[0].name.as_str(), "Beta");

    // An upload re-inserts Alpha ahead of it: the list grows, Beta shifts back 0 → 1.
    app.set_routes_with_ids(&routes, &IDS3);
    let active = app.activity.active_route.expect("still navigating");
    assert_eq!(app.routes()[active].name.as_str(), "Beta", "grown list: still the same route");
}

/// The *navigated* route vanishing unloads navigation — `None`, never a neighbour aliased in by
/// the index shift.
#[test]
fn rescan_unloads_a_vanished_active_route() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    let routes = test_routes();
    app.set_routes_with_ids(&routes, &IDS3);
    app.activity.active_route = Some(1); // Beta
    let keep = [routes[0].clone(), routes[2].clone()]; // Beta deleted
    app.set_routes_with_ids(&keep, &[IDS3[0], IDS3[2]]);
    assert_eq!(app.activity.active_route, None, "the deleted route unloads; Gamma is not aliased in");
}

/// An open Route menu across a rescan: the highlight follows the previously-highlighted route's
/// identity to its new row.
#[test]
fn rescan_follows_the_open_route_menu_selection() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    let routes = test_routes();
    app.set_routes_with_ids(&routes, &IDS3);
    app.apply_gesture(Gesture::Press); // Home → Menu (Routes selected)
    app.apply_gesture(Gesture::Press); // Menu → Route menu
    app.apply_gesture(Gesture::Turn(1)); // highlight Beta
    app.set_routes_with_ids(&routes[1..], &IDS3[1..]); // Alpha deleted under the open menu
    app.apply_gesture(Gesture::Press); // open the highlighted route
    let active = app.activity.active_route.expect("the overview loaded the highlighted route");
    assert_eq!(app.routes()[active].name.as_str(), "Beta", "the highlight followed Beta to its new row");
}

/// A vanished highlight falls back to the nearest row (clamped), never a dangling index.
#[test]
fn rescan_clamps_a_vanished_menu_selection() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    let routes = test_routes();
    app.set_routes_with_ids(&routes, &IDS3);
    app.apply_gesture(Gesture::Press); // Home → Menu (Routes selected)
    app.apply_gesture(Gesture::Press); // Menu → Route menu
    app.apply_gesture(Gesture::Turn(2)); // highlight Gamma (last row)
    app.set_routes_with_ids(&routes[..2], &IDS3[..2]); // Gamma deleted
    app.apply_gesture(Gesture::Press); // open whatever is highlighted now
    let active = app.activity.active_route.expect("a clamped highlight still opens a real route");
    assert_eq!(app.routes()[active].name.as_str(), "Beta", "the highlight clamped to the last row");
}

// ==================== on-device route delete (epic #447, P6) ====================
//
// The Route menu's hold-to-delete footer records a delete request the host drains as the route's
// durable object id; after the delete + rescan, P3's remap keeps `active_route` + the highlight on
// the right routes.

/// Holding the encoder over the highlighted route records a delete request the host drains as that
/// route's **durable object id** (not its index) — the id lookup is `App`'s.
#[test]
fn hold_delete_requests_the_highlighted_route_id() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.set_routes_with_ids(&test_routes(), &IDS3); // ids 10, 20, 30
    app.apply_gesture(Gesture::Press); // Home → Menu (Routes selected)
    app.apply_gesture(Gesture::Press); // Menu → Route menu
    app.apply_gesture(Gesture::Turn(1)); // highlight Beta (id 20)
    assert!(!app.has_route_delete(), "no request until the hold completes");
    app.apply_gesture(Gesture::Hold); // guarded hold = delete Beta
    assert!(app.has_route_delete(), "the hold recorded a delete request");
    assert_eq!(app.take_route_delete(), Some(20), "drained as Beta's durable id, not its index");
    assert_eq!(app.take_route_delete(), None, "the one-shot drains");
}

/// The DoD case: deleting a *non-highlighted* route (the host removes it + re-feeds the catalog)
/// keeps the highlight on the same route, by identity — not on whatever slid into its old row.
#[test]
fn deleting_a_non_highlighted_route_keeps_the_highlight_by_id() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    let routes = test_routes(); // Alpha(10), Beta(20), Gamma(30)
    app.set_routes_with_ids(&routes, &IDS3);
    app.apply_gesture(Gesture::Press); // Home → Menu (Routes selected)
    app.apply_gesture(Gesture::Press); // Menu → Route menu
    app.apply_gesture(Gesture::Turn(1)); // highlight Beta (id 20)

    // Simulate the host handling a delete of Alpha (a *different* route) — remove it and rescan.
    let keep = [routes[1].clone(), routes[2].clone()];
    app.set_routes_with_ids(&keep, &[IDS3[1], IDS3[2]]); // Beta shifts 1 → 0

    // Pressing opens the highlighted route: still Beta, now at its new row.
    app.apply_gesture(Gesture::Press);
    let active = app.activity.active_route.expect("the overview loaded the highlighted route");
    assert_eq!(app.routes()[active].name.as_str(), "Beta", "the highlight stayed on Beta across the delete");
}

/// Deleting the *highlighted* route moves the highlight sanely (clamped to the nearest surviving
/// row), never a dangling index — the host removed the route the menu was pointing at.
#[test]
fn deleting_the_highlighted_route_moves_the_highlight_sanely() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    let routes = test_routes();
    app.set_routes_with_ids(&routes, &IDS3);
    app.apply_gesture(Gesture::Press); // Home → Menu (Routes selected)
    app.apply_gesture(Gesture::Press); // Menu → Route menu
    app.apply_gesture(Gesture::Turn(2)); // highlight Gamma (id 30, last row)
    app.apply_gesture(Gesture::Hold); // request its delete
    assert_eq!(app.take_route_delete(), Some(30));

    // The host deletes Gamma and re-feeds the catalog: the highlight clamps to the new last row.
    app.set_routes_with_ids(&routes[..2], &IDS3[..2]);
    app.apply_gesture(Gesture::Press); // open whatever is highlighted now
    let active = app.activity.active_route.expect("a clamped highlight still opens a real route");
    assert_eq!(app.routes()[active].name.as_str(), "Beta", "the highlight clamped to the surviving last row");
}

/// Ride to the Map on Alpha, then open the swap prompt for Gamma — the shared mid-ride setup for
/// the pending-swap remap cases.
fn app_with_pending_swap_on_gamma() -> App {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.set_routes_with_ids(&test_routes(), &IDS3);
    app.apply_gesture(Gesture::Press); // Home → Menu (Routes selected)
    app.apply_gesture(Gesture::Press); // Menu → Route menu
    app.apply_gesture(Gesture::Press); // Alpha → overview
    app.apply_gesture(Gesture::Press); // START RIDE → Map, session running
    assert_eq!(app.mode(), Mode::Riding);
    assert_eq!(app.activity.active_route, Some(0));
    app.apply_gesture(Gesture::BackHold); // Map → Menu
    app.apply_gesture(Gesture::Press); // Routes station → Route menu
    app.apply_gesture(Gesture::Turn(2)); // highlight Gamma
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
    let active = app.activity.active_route.expect("swap navigated");
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
    let active = app.activity.active_route.expect("the original navigation is untouched");
    assert_eq!(app.routes()[active].name.as_str(), "Alpha", "still navigating the original route");
}

/// The store-changed drain: `take_store_changed` returns the pending count once and resets it —
/// the edge the board's live rescan keys on.
#[test]
fn take_store_changed_drains_the_pending_count() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    assert_eq!(app.take_store_changed(), 0);
    app.notify_store_changed();
    app.notify_store_changed();
    assert_eq!(app.store_changed_pending(), 2, "the read-only observer still sees the count");
    assert_eq!(app.take_store_changed(), 2);
    assert_eq!(app.store_changed_pending(), 0, "drained");
}

/// Feed a single encoder press (down+up within the threshold) to the app.
fn press(app: &mut App) {
    let mut s = keys(&[
        InputEvent::Button(ButtonEvent::Down(Button::Encoder)),
        InputEvent::Button(ButtonEvent::Up(Button::Encoder)),
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
    app.activity.start_session(); // a tracking ride, so the map's press pauses (not the browse-map start card)

    // Riding: sample the (blue sea) backdrop at a point clear of the map chrome — the clock digits
    // end ~y28, the bottom-centre "No GPS Fix" chip band starts ~y74 on the 120px test frame, and
    // the scale bar + label own the bottom-left, so mid-right between them is bare map.
    let map = render(&mut app, &bytes);
    let backdrop = map.get(95, 45);

    // A press (Down+Up within the threshold) pauses into the Paused page.
    let mut press = keys(&[
        InputEvent::Button(ButtonEvent::Down(Button::Encoder)),
        InputEvent::Button(ButtonEvent::Up(Button::Encoder)),
    ]);
    app.handle_input(InputClock(0), &mut press);
    assert_eq!(app.mode(), Mode::Paused, "press paused the ride");

    // Now the same point carries the parchment Paused page, not the map.
    let paused = render(&mut app, &bytes);
    let page = paused.get(95, 45);
    assert_ne!(page, backdrop, "pausing replaced the view");
    assert!(page.r() > backdrop.r(), "the parchment page is lighter than the sea backdrop");
}

fn render(app: &mut App, bytes: &[u8]) -> Buf {
    app.tick(
        RideClock(0),
        Sensors {
            loc: &mut NoFix,
            altimeter: None,
            temperature: None,
            clock: None,
            compass: None,
            track: None,
            fuel: None,
        },
        None,
    );
    let cache = MapCache::new();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("valid v7 file");
    let reader = Reader::new(&src, &tables, &cache);
    let mut buf = Buf::new(120, 120);
    app.render_frame(&mut buf, &reader, None, 120.0, 120.0, |c| {
        let (r, g, b) = rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
    buf
}

// Pan mode (a Map sub-mode driven by the shared `AppState::pan`): enter/exit, the axis + orientation
// toggles, panning, and the camera freeze.

/// `hold` on the Follow map enters pan: the camera detaches (Free) and a pan state
/// appears — axis Vertical, orientation matching the map (here north-up).
#[test]
fn map_hold_enters_pan_mode() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    let t = MapScreen::new().handle(Gesture::Hold, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::None));
    let pan = st.pan.expect("hold enters pan");
    assert_eq!(pan.axis, PanAxis::Vertical);
    assert!(pan.north_up, "a north-up map enters pan north-up");
    assert_eq!(st.mode, CameraMode::Free, "the camera detaches while panning");
}

/// While panning, a fresh fix no longer recenters the frozen camera (but is still
/// recorded for the marker).
#[test]
fn pan_freezes_camera_against_fixes() {
    let (mut st, _act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    st.enter_pan();
    st.update(&mut ReplayFix(Some(Fix::at(5000, 7000))));
    assert_eq!((st.cam_lon, st.cam_lat), (0, 0), "the frozen camera ignores the fix");
    assert_eq!(st.user_fix.map(|f| (f.lon, f.lat)), Some((7000, 5000)), "but the fix is recorded");
}

/// `turn` moves the frozen camera along the active axis: a positive detent on a
/// north-up map pans up (+latitude), leaving longitude alone, and reversing returns
/// to the start (within microdegree rounding).
#[test]
fn pan_turn_moves_camera_along_axis() {
    let (mut st, mut act) = (AppState::new(0, 0, 4.0), Activity::new(Mode::Riding));
    st.enter_pan(); // north-up (heading_up defaults false)
    MapScreen::new().handle(Gesture::Turn(1), &mut ctx(&mut st, &mut act));
    assert!(st.cam_lat > 0, "a positive detent pans up = +latitude");
    assert_eq!(st.cam_lon, 0, "the vertical axis leaves longitude unchanged");
    MapScreen::new().handle(Gesture::Turn(-1), &mut ctx(&mut st, &mut act));
    assert!(st.cam_lat.abs() <= 1 && st.cam_lon.abs() <= 1, "reversing returns to the start (±1 µdeg)");
}

/// `press` toggles the pan axis; `hold` flips N-up ↔ heading-up and freezes the new
/// angle, so the map orientation never drifts while panning.
#[test]
fn pan_press_toggles_axis_hold_toggles_orientation() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    st.heading_up = true;
    st.user_fix = Some(Fix { lat: 0, lon: 0, course: Some(90.0), speed_mps: Some(5.0) });
    st.enter_pan();
    assert!(!st.pan.unwrap().north_up, "a heading-up map enters pan heading-up");
    let rot0 = st.viewport(240.0, 320.0).course_rad;
    assert!((rot0 - std::f32::consts::FRAC_PI_2).abs() < 1e-3, "frozen at the 90° course");

    MapScreen::new().handle(Gesture::Press, &mut ctx(&mut st, &mut act));
    assert_eq!(st.pan.unwrap().axis, PanAxis::Horizontal);

    MapScreen::new().handle(Gesture::Hold, &mut ctx(&mut st, &mut act));
    assert!(st.pan.unwrap().north_up, "hold flips to north-up");
    assert!(st.viewport(240.0, 320.0).course_rad.abs() < 1e-6, "north-up = 0 rotation");

    MapScreen::new().handle(Gesture::Hold, &mut ctx(&mut st, &mut act));
    assert!(!st.pan.unwrap().north_up, "hold flips back to heading-up");
    assert!(
        (st.viewport(240.0, 320.0).course_rad - std::f32::consts::FRAC_PI_2).abs() < 1e-3,
        "and re-freezes the course"
    );
}

/// `back` recenters on the rider but stays in pan; `back-hold` exits to Follow and
/// does *not* fall through to the global `back-hold` = Menu.
#[test]
fn pan_back_recenters_and_back_hold_exits() {
    let (mut st, mut act) = (AppState::new(0, 0, 4.0), Activity::new(Mode::Riding));
    st.user_fix = Some(Fix::at(5000, 7000));
    st.enter_pan();
    MapScreen::new().handle(Gesture::Turn(2), &mut ctx(&mut st, &mut act)); // pan away
    assert_ne!((st.cam_lon, st.cam_lat), (7000, 5000));

    let t = MapScreen::new().handle(Gesture::Back, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::None));
    assert_eq!((st.cam_lon, st.cam_lat), (7000, 5000), "back recenters on the fix");
    assert!(st.pan.is_some(), "back stays in pan");

    let t = MapScreen::new().handle(Gesture::BackHold, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::None), "back-hold doesn't open the Menu while panning");
    assert!(st.pan.is_none(), "back-hold exits pan");
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

    let mut app = App::new_idle(AppState::new(0, 0, 0.05)); // [Home]
    app.set_nav_profiles(tables.nav_profiles()); // the host's map-load mirror
    assert_eq!(app.nav_profiles().len(), 4, "all four §8.6 names resident");
    assert_eq!(app.nav_profiles().name(2), Some("MTB"));

    // Home → Menu → Settings list → the Bike type row (index 2) → its screen.
    app.apply_gesture(Gesture::BackHold); // → Menu
    app.apply_gesture(Gesture::Turn(-1)); // compass: one ccw detent to Settings
    app.apply_gesture(Gesture::Press); // → Settings list
    app.apply_gesture(Gesture::Turn(2)); // Date & Time → Units → Bike type
    app.apply_gesture(Gesture::Press); // → Bike type screen
    assert!(matches!(app.top_screen(), obc_app::Screen::BikeType(_)), "navigated to the Bike type screen");

    // Two detents: Road → Gravel → MTB.
    app.apply_gesture(Gesture::Turn(1));
    app.apply_gesture(Gesture::Turn(1));
    assert_eq!(app.settings().bike_profile_idx, 2, "two detents from Road land on MTB");

    // The save is debounced to leaving the settings subtree (Bike type → Settings list → Menu).
    assert!(!app.take_settings_dirty(), "no save cue while still inside Settings");
    app.apply_gesture(Gesture::Back);
    app.apply_gesture(Gesture::Back); // → Menu (out of the subtree)
    assert!(app.take_settings_dirty(), "leaving Settings fires the debounced save");

    // Simulated reboot: the persisted blob seeds a fresh App (the boot path of both hosts).
    let blob = obc_app::settings::encode(app.settings());
    let restored = obc_app::settings::decode(&blob).expect("clean blob decodes");
    let mut app2 = App::new_idle(AppState::new(0, 0, 0.05));
    app2.set_settings(restored);
    assert_eq!(app2.settings().bike_profile_idx, 2, "the bike profile survives the reboot");
}

/// A stored index past the loaded map's profile count (a stale setting against a smaller map)
/// renders the honest `Profile N` fallback — matching the router's own profile-0 fallback (N3) —
/// and an in-range index renders the map's name. Pinned through the App's resident mirror, i.e.
/// exactly what the Bike-type row and the overview label draw.
#[test]
fn bike_type_out_of_range_renders_fallback() {
    let bytes = build_min_obcm_profiles(0, &["Road", "MTB"]);
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).expect("valid fixture");

    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    app.set_nav_profiles(tables.nav_profiles());
    app.set_settings(Settings { bike_profile_idx: 7, ..Settings::default() }); // stale: map has 2

    let mut label: heapless::String<20> = heapless::String::new();
    app.nav_profiles().write_label(app.settings().bike_profile_idx, &mut label);
    assert_eq!(label.as_str(), "Profile 7", "an out-of-range index shows the generic fallback");

    let mut ok: heapless::String<20> = heapless::String::new();
    app.nav_profiles().write_label(1, &mut ok);
    assert_eq!(ok.as_str(), "MTB", "an in-range index shows the map's name");
}
