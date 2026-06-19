//! Screen-stack tests: navigation [`Transition`]s per gesture, the guarded-action
//! "needs a completed hold" rule, the stack discipline ([`apply`]), and a render
//! snapshot proving Ride control composites over the map. Mirrors the style of
//! `obc-render/tests/priority.rs` (feed inputs, assert the outcome) and
//! `obc-app/tests/marker.rs` (render into a tiny `DrawTarget`).

use std::collections::VecDeque;

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use obc_app::activity::Activity;
use obc_app::screen::{
    apply, Ctx, HomeScreen, MapScreen, MenuScreen, RideControl, RouteMenuScreen, RouteSwapScreen, Screen,
    Stack, Transition,
};
use obc_app::{App, AppState, Button, ButtonEvent, CameraMode, Fix, Gesture, InputClock, InputEvent, InputSource, LocationSource, Mode, PanAxis, RideClock, RouteSummary, Sensors, TrackAction};
use obc_reader::{rgb565_to_rgb888, BBox, Reader};

/// A handle [`Ctx`] over freshly-made state/activity for a one-gesture test. Most
/// screens ignore the catalog; the Route-menu tests pass their own via [`route_ctx`].
fn ctx<'a>(state: &'a mut AppState, activity: &'a mut Activity) -> Ctx<'a> {
    Ctx { state, activity, routes: &[], now_ms: 0 }
}

/// A handle [`Ctx`] carrying a route catalog, for the Route-menu tests.
fn route_ctx<'a>(state: &'a mut AppState, activity: &'a mut Activity, routes: &'a [RouteSummary]) -> Ctx<'a> {
    Ctx { state, activity, routes, now_ms: 0 }
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

// ---------------------------------------------------------------------------
// Per-gesture navigation transitions.
// ---------------------------------------------------------------------------

#[test]
fn map_press_pauses_into_ride_control() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
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

// ---------------------------------------------------------------------------
// The Home → Route menu → Map flow.
// ---------------------------------------------------------------------------

#[test]
fn home_press_opens_the_route_menu() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    let t = HomeScreen::new().handle(Gesture::Press, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::Push(Screen::RouteMenu(_))));
}

#[test]
fn route_menu_loads_the_selected_route_and_opens_the_map() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
    st.mode = CameraMode::Free; // the map-viewer default; loading must flip to Follow
    st.heading_up = false;
    let routes = test_routes();
    let mut rm = RouteMenuScreen::new();
    rm.handle(Gesture::Turn(1), &mut route_ctx(&mut st, &mut act, &routes)); // highlight route 1
    let t = rm.handle(Gesture::Press, &mut route_ctx(&mut st, &mut act, &routes));
    assert!(matches!(t, Transition::Root(Screen::Map(_))), "loading lands on a clean [Home, Map]");
    assert_eq!(act.mode, Mode::Riding, "loading starts tracking");
    assert_eq!(act.active_route, Some(1), "the selected route is the active one");
    assert!(act.is_tracking(), "loading from Idle begins a tracking session");
    // Loading drops into the riding view: follow + heading-up, seeded at the start.
    assert_eq!(st.mode, CameraMode::Follow);
    assert!(st.heading_up);
    assert_eq!((st.cam_lon, st.cam_lat), (100, 100), "camera seeded at the route start");
    assert!(st.zoom > 0.2 && st.zoom < 0.25, "~0.5 m/px riding zoom, got {}", st.zoom);
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

// ---------------------------------------------------------------------------
// Loading a route mid-session: the swap / save prompt, Finish / Discard.
// ---------------------------------------------------------------------------

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
    // End to end through `App`: Idle Home → press → Route menu → press → riding Map.
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    app.set_routes(&test_routes());
    assert_eq!(app.mode(), Mode::Idle);
    press(&mut app); // Home → Route menu
    assert_eq!(app.mode(), Mode::Idle, "opening the route list doesn't start riding yet");
    press(&mut app); // Route menu → load route 0 → Map
    assert_eq!(app.mode(), Mode::Riding);
    assert_eq!(app.activity.active_route, Some(0));
}

/// Feed a single encoder press (down+up within the threshold) to the app.
fn press(app: &mut App) {
    let mut s = Script(VecDeque::from(vec![
        InputEvent::Button(ButtonEvent::Down(Button::Encoder)),
        InputEvent::Button(ButtonEvent::Up(Button::Encoder)),
    ]));
    app.handle_input(InputClock(0), &mut s);
}

// ---------------------------------------------------------------------------
// Stack discipline.
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Render snapshot: the map, and Ride control composited over it.
// ---------------------------------------------------------------------------

/// One scripted raw input event per `poll` — drives [`App::handle_input`].
struct Script(VecDeque<InputEvent>);
impl InputSource for Script {
    fn poll(&mut self) -> Option<InputEvent> {
        self.0.pop_front()
    }
}

/// A no-fix location source (the marker stays off so it can't confuse the snapshot).
struct NoFix;
impl LocationSource for NoFix {
    fn poll(&mut self) -> Option<Fix> {
        None
    }
}

#[test]
fn ride_control_composites_over_the_map() {
    let bytes = build_min_obcm(0xF800);
    let mut app = App::new(AppState::new(0, 0, 0.05));

    // Riding: the center is the (blue sea) backdrop.
    let map = render(&mut app, &bytes);
    let backdrop = map.get(60, 60);

    // A press (Down+Up within the threshold) pauses into Ride control.
    let mut press = Script(VecDeque::from(vec![
        InputEvent::Button(ButtonEvent::Down(Button::Encoder)),
        InputEvent::Button(ButtonEvent::Up(Button::Encoder)),
    ]));
    app.handle_input(InputClock(0), &mut press);
    assert_eq!(app.mode(), Mode::Paused, "press paused the ride");

    // Now the center carries the parchment Ride-control panel, not the backdrop.
    let paused = render(&mut app, &bytes);
    let panel = paused.get(60, 60);
    assert_ne!(panel, backdrop, "the overlay changed the center");
    assert!(panel.r() > backdrop.r(), "parchment panel is lighter than the sea backdrop");
}

// --- tiny render harness (mirrors marker.rs) ---

fn render(app: &mut App, bytes: &[u8]) -> Buf {
    app.tick(RideClock(0), Sensors { loc: &mut NoFix, altimeter: None, track: None }, None);
    let reader = Reader::new(bytes).expect("valid v5 file");
    let mut buf = Buf::new(120, 120);
    app.render_frame(&mut buf, &reader, None, 120.0, 120.0, |c| {
        let (r, g, b) = rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
    buf
}

struct Buf {
    w: i32,
    h: i32,
    px: Vec<Rgb888>,
}
impl Buf {
    fn new(w: i32, h: i32) -> Self {
        Buf { w, h, px: vec![Rgb888::BLACK; (w * h) as usize] }
    }
    fn get(&self, x: i32, y: i32) -> Rgb888 {
        self.px[(y * self.w + x) as usize]
    }
    fn put(&mut self, x: i32, y: i32, c: Rgb888) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = c;
        }
    }
}
impl OriginDimensions for Buf {
    fn size(&self) -> Size {
        Size::new(self.w as u32, self.h as u32)
    }
}
impl DrawTarget for Buf {
    type Color = Rgb888;
    type Error = core::convert::Infallible;
    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            self.put(p.x, p.y, c);
        }
        Ok(())
    }
    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let clip = area.intersection(&self.bounding_box());
        if let Some(br) = clip.bottom_right() {
            for y in clip.top_left.y..=br.y {
                for x in clip.top_left.x..=br.x {
                    self.put(x, y, color);
                }
            }
        }
        Ok(())
    }
}

/// A minimal valid v5 file: one sea-backdrop style, one empty LOD leaf, no chunks.
/// The map renders as a flat backdrop, so the only non-backdrop pixels come from a
/// screen drawn on top. (Same builder as `marker.rs`.)
fn build_min_obcm(marker: u16) -> Vec<u8> {
    let style_off: u32 = 32;
    let mut styles = vec![1u8];
    styles.push(1);
    styles.push(0);
    styles.extend_from_slice(&0x001Fu16.to_le_bytes());
    styles.push(1);
    styles.push(0);

    let lod_tab_off = style_off as usize + styles.len();
    let index_off = lod_tab_off + 18;

    let mut table = Vec::new();
    table.extend_from_slice(&f32::INFINITY.to_le_bytes());
    table.extend_from_slice(&(index_off as u32).to_le_bytes());
    table.extend_from_slice(&1u32.to_le_bytes());
    table.extend_from_slice(&16u16.to_le_bytes());
    table.extend_from_slice(&0u32.to_le_bytes());

    let index = 0x7FFF_FFFFu32.to_le_bytes();

    let mut f = Vec::new();
    f.extend_from_slice(b"OBCM");
    f.push(5);
    for v in [-1000i32, -1000, 1000, 1000] {
        f.extend_from_slice(&v.to_le_bytes());
    }
    f.extend_from_slice(&style_off.to_le_bytes());
    f.push(1);
    f.extend_from_slice(&(lod_tab_off as u32).to_le_bytes());
    f.extend_from_slice(&marker.to_le_bytes());
    f.extend_from_slice(&styles);
    f.extend_from_slice(&table);
    f.extend_from_slice(&index);
    f
}

// ---------------------------------------------------------------------------
// Pan mode (a Map sub-mode driven by the shared `AppState::pan`): enter/exit,
// the axis + orientation toggles, panning, and the camera freeze.
// ---------------------------------------------------------------------------

/// A location source that always returns the same fix (for the freeze test).
struct OneFix(Fix);
impl LocationSource for OneFix {
    fn poll(&mut self) -> Option<Fix> {
        Some(self.0)
    }
}

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
    st.update(&mut OneFix(Fix::at(5000, 7000)));
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
    assert!((st.viewport(240.0, 320.0).course_rad - std::f32::consts::FRAC_PI_2).abs() < 1e-3, "and re-freezes the course");
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
