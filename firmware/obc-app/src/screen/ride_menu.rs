//! The mid-ride compass menu (epic #789): five fixed stations with the same bezel, needle sweep and
//! steps as the main Menu. **Up ahead** opens the merged waypoint + corridor-POI timeline (epic
//! #946, U3 — it replaced the plain waypoint list of #787), Detour opens the rejoin chooser
//! (#788 → routed detour #882), and POIs, Routes and Main menu open their existing screens.

use core::fmt::Write as _;

use obc_render::Surface;

use crate::input::Gesture;
use crate::Msg;

use super::menu::{CompassDial, CompassIcons};
use super::{
    Ctx, DetourScreen, MenuScreen, PoiMenuScreen, Render, RouteMenuScreen, Screen, ScreenTick, Transition,
    UpAheadScreen,
};

/// The fixed five-station ride-menu ring (epic #789's locked count — independent of the main
/// menu's, which grew a Weather station in WX11; the shared dial takes the count per call).
const N_ITEMS: usize = 5;

/// The fixed ride-menu ring. The selected station always starts at Up ahead (north); keeping all
/// five entries present on route-less rides preserves the dial geometry and muscle memory.
#[derive(Debug, Default)]
pub struct RideMenuScreen {
    dial: CompassDial,
}

impl RideMenuScreen {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => self.dial.step(n, N_ITEMS),
            Gesture::Press => match self.dial.selected() {
                // The timeline anchors its corridor snapshot on live progress **at entry**, takes
                // the rider's source scope from Ride settings at the same moment (U4), and homes
                // its cursor on the first entry still ahead (epic #946, U3); a route-less ride
                // simply opens on its empty state.
                0 => Transition::Push(Screen::UpAhead(UpAheadScreen::new(
                    cx.activity.progress_m,
                    cx.settings.up_ahead_source,
                ))),
                // Replace the transient compass, so chooser Press/Back can Pop once to the exact
                // caller (Map, Statistics/Climb, or paused Ride control). A dimmed station (no
                // nav graph / no route / off-route) never transitions.
                1 if detour_available(cx.activity, cx.state.has_nav_graph) => {
                    Transition::Replace(Screen::Detour(DetourScreen::new(cx.activity)))
                }
                1 => Transition::None,
                2 => Transition::Push(Screen::PoiMenu(PoiMenuScreen::new())),
                3 => Transition::Push(Screen::RouteMenu(RouteMenuScreen::new())),
                _ => Transition::Push(Screen::Menu(MenuScreen::new())),
            },
            Gesture::Back => Transition::Pop,
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        self.dial.tick_timers(now_ms)
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let items: [&str; N_ITEMS] = [
            rx.t(Msg::RideMenuUpAhead),
            rx.t(Msg::RideMenuDetour),
            rx.t(Msg::MenuPois),
            rx.t(Msg::MenuRoutes),
            rx.t(Msg::RideMenuMainMenu),
        ];
        let mut batt: heapless::String<8> = heapless::String::new();
        let device = rx.state.device_status();
        let _ = write!(batt, "{}%", device.battery_pct);
        self.dial.draw(
            cv,
            rx.w,
            rx.h,
            device.ble_connected(),
            &batt,
            rx.t(Msg::RideMenuTitle),
            &items,
            CompassIcons::Ride {
                route_loaded: rx.activity.active_route.is_some(),
                detour_available: detour_available(rx.activity, rx.state.has_nav_graph),
            },
        );
    }
}

/// Whether the Detour station is actionable (#882): a route is loaded, the map has a nav graph,
/// and the rider is on the route (the corridor anchors on live progress, which off-route
/// freezes). Shared by the station's Press gate and its dimming.
fn detour_available(activity: &crate::activity::Activity, has_nav_graph: bool) -> bool {
    activity.active_route.is_some() && has_nav_graph && !activity.off_route
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::screen::test_ctx;
    use crate::screen::{apply, Stack};
    use crate::{AppState, Settings};

    fn station_ctx(activity: &mut Activity) -> Ctx<'_> {
        let state = Box::leak(Box::new(AppState::new(0, 0, 1.0)));
        let settings = Box::leak(Box::new(Settings::default()));
        test_ctx(state, activity, settings)
    }

    fn run(scr: &mut RideMenuScreen, g: Gesture) -> Transition {
        let mut state = AppState::new(0, 0, 1.0);
        // A routed, nav-graph ride so every station (incl. the gated Detour, #882) is actionable.
        state.has_nav_graph = true;
        let mut activity = Activity::new(Mode::Riding);
        activity.active_route = Some(0);
        let mut settings = Settings::default();
        let mut cx = test_ctx(&mut state, &mut activity, &mut settings);
        scr.handle(g, &mut cx)
    }

    #[test]
    fn fixed_ring_dispatches_all_five_stations_in_order() {
        fn press_at(step: i32) -> Transition {
            let mut scr = RideMenuScreen::new();
            run(&mut scr, Gesture::Step(step));
            run(&mut scr, Gesture::Press)
        }
        assert!(matches!(press_at(0), Transition::Push(Screen::UpAhead(_))));
        assert!(matches!(press_at(1), Transition::Replace(Screen::Detour(_))));
        assert!(matches!(press_at(2), Transition::Push(Screen::PoiMenu(_))));
        assert!(matches!(press_at(3), Transition::Push(Screen::RouteMenu(_))));
        assert!(matches!(press_at(-1), Transition::Push(Screen::Menu(_))));
    }

    #[test]
    fn ring_wrap_and_back_are_stable() {
        let mut scr = RideMenuScreen::new();
        run(&mut scr, Gesture::Step(5));
        assert!(matches!(run(&mut scr, Gesture::Press), Transition::Push(Screen::UpAhead(_))));
        assert!(matches!(run(&mut RideMenuScreen::new(), Gesture::Back), Transition::Pop));
    }

    #[test]
    fn route_less_ring_dims_only_the_route_dependent_stations() {
        let route_less = CompassIcons::Ride { route_loaded: false, detour_available: false };
        assert!(!route_less.enabled(0));
        assert!(!route_less.enabled(1));
        assert!((2..N_ITEMS).all(|i| route_less.enabled(i)));
        // A loaded route on a nav-graph map lights everything; a graph-less (or off-route) map
        // dims only the Detour station (#882).
        let all = CompassIcons::Ride { route_loaded: true, detour_available: true };
        assert!((0..N_ITEMS).all(|i| all.enabled(i)));
        let no_nav = CompassIcons::Ride { route_loaded: true, detour_available: false };
        assert!(no_nav.enabled(0) && !no_nav.enabled(1));
        assert!((2..N_ITEMS).all(|i| no_nav.enabled(i)));
    }

    #[test]
    fn ride_menu_uses_the_shared_needle_timer() {
        let mut scr = RideMenuScreen::new();
        run(&mut scr, Gesture::Step(1));
        assert_eq!(scr.tick_timers(0).next_wake_ms, Some(16));
        let tick = scr.tick_timers(16);
        assert!(tick.changed);
        assert_eq!(tick.next_wake_ms, Some(16));
    }

    /// The north station opens the timeline anchored on **live progress at entry** (the corridor
    /// snapshot's anchor, epic #946 U3) and leaves the recording session exactly as it found it.
    #[test]
    fn up_ahead_station_anchors_on_live_progress_and_preserves_activity() {
        let mut activity = Activity::new(Mode::Paused);
        activity.start_session();
        activity.mode = Mode::Paused;
        activity.progress_m = 4_200;
        let session = activity.session;
        let mut menu = RideMenuScreen::new();
        match menu.handle(Gesture::Press, &mut station_ctx(&mut activity)) {
            Transition::Push(Screen::UpAhead(screen)) => {
                let key = screen.corridor_key().expect("the default source scope wants a snapshot");
                assert_eq!(key.anchor_m, 4_200, "the snapshot anchors where the rider is");
                assert_eq!(key.filter, obc_reader::PoiCategorySet::ALL, "the list opens on Everything, every time");
            }
            _ => panic!("the Up ahead station did not push its timeline"),
        }
        assert_eq!(activity.mode, Mode::Paused, "opening ride chrome never resumes/pauses the session");
        assert_eq!(activity.session, session, "opening the list never starts a new session");
    }

    #[test]
    fn skip_replace_then_pop_preserves_each_caller_and_activity() {
        fn round_trip(caller: Screen, paused: bool, caller_matches: impl Fn(&Screen) -> bool) {
            let mut activity = Activity::new(if paused { Mode::Paused } else { Mode::Riding });
            activity.active_route = Some(0);
            activity.route_total_m = 2_000;
            activity.start_session();
            let mode = activity.mode;
            let session = activity.session();
            let mut stack = Stack::new();
            assert!(stack.push(caller).is_ok());
            assert!(stack.push(Screen::RideMenu(RideMenuScreen::new())).is_ok());
            apply(&mut stack, Transition::Replace(Screen::Detour(DetourScreen::new(&activity))));
            assert!(matches!(stack.last(), Some(Screen::Detour(_))));
            apply(&mut stack, Transition::Pop);
            assert!(caller_matches(stack.last().unwrap()));
            assert_eq!(activity.mode, mode);
            assert_eq!(activity.session(), session);
        }

        round_trip(Screen::Map(super::super::MapScreen::new()), false, |s| matches!(s, Screen::Map(_)));
        round_trip(Screen::Statistics(super::super::StatisticsScreen::new()), false, |s| {
            matches!(s, Screen::Statistics(_))
        });
        round_trip(Screen::RideControl(super::super::RideControl::new()), true, |s| {
            matches!(s, Screen::RideControl(_))
        });
    }
}
