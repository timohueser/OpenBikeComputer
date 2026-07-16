//! The mid-ride compass menu (epic #789, RM1): five fixed stations with the same bezel, needle
//! sweep and detents as the main Menu. Waypoints and Skip ahead are navigation-safe stubs in this
//! slice; POIs, Routes and Main menu open their existing screens.

use core::fmt::Write as _;

use obc_render::Surface;

use crate::input::Gesture;
use crate::Msg;

use super::menu::{CompassDial, CompassIcons, N_ITEMS};
use super::{
    empty_state, title_frame, Ctx, MenuScreen, PoiMenuScreen, Render, RouteMenuScreen, Screen, ScreenTick, Transition,
};

/// The fixed ride-menu ring. The selected station always starts at Waypoints (north); keeping all
/// five entries present on route-less rides preserves the dial geometry and muscle memory.
#[derive(Debug, Default)]
pub struct RideMenuScreen {
    dial: CompassDial,
}

impl RideMenuScreen {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => self.dial.turn(n),
            Gesture::Press => match self.dial.selected() {
                0 => Transition::Push(Screen::RideWaypoints(RideWaypointsScreen::new())),
                1 => Transition::Push(Screen::SkipAhead(SkipAheadScreen::new())),
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
            rx.t(Msg::RideMenuWaypoints),
            rx.t(Msg::RideMenuSkipAhead),
            rx.t(Msg::MenuPois),
            rx.t(Msg::MenuRoutes),
            rx.t(Msg::RideMenuMainMenu),
        ];
        let mut batt: heapless::String<8> = heapless::String::new();
        let _ = write!(batt, "{}%", rx.state.battery_pct);
        self.dial.draw(
            cv,
            rx.w,
            rx.h,
            rx.state.ble_connected(),
            &batt,
            rx.t(Msg::RideMenuTitle),
            &items,
            CompassIcons::Ride { route_loaded: rx.activity.active_route.is_some() },
        );
    }
}

/// Waypoints station placeholder. It is a real child screen now so the finished waypoint browser
/// can replace the body without changing ride-menu navigation or stack behavior.
#[derive(Debug, Default)]
pub struct RideWaypointsScreen;

impl RideWaypointsScreen {
    pub fn new() -> Self {
        Self
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        stub_handle(g)
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        draw_stub(cv, rx, Msg::RideMenuWaypoints);
    }
}

/// Skip-ahead station placeholder. RM2 can grow this state into the distance chooser while keeping
/// the station and caller contract stable.
#[derive(Debug, Default)]
pub struct SkipAheadScreen;

impl SkipAheadScreen {
    pub fn new() -> Self {
        Self
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        stub_handle(g)
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        draw_stub(cv, rx, Msg::RideMenuSkipAhead);
    }
}

fn stub_handle(g: Gesture) -> Transition {
    match g {
        Gesture::Back => Transition::Pop,
        Gesture::Turn(_) | Gesture::Press | Gesture::Hold | Gesture::BackHold => Transition::None,
    }
}

fn draw_stub(cv: &mut impl Surface, rx: &mut Render, title: Msg) {
    title_frame(cv, rx.w, rx.h, rx.t(title), "");
    let (body, hint) = if rx.activity.active_route.is_some() {
        (rx.t(Msg::RideMenuComingSoon), "")
    } else {
        (rx.t(Msg::RideMenuNoRoute), "")
    };
    empty_state(cv, rx.w, rx.h, body, hint);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::{AppState, Settings};

    fn run(scr: &mut RideMenuScreen, g: Gesture) -> Transition {
        let mut state = AppState::new(0, 0, 1.0);
        let mut activity = Activity::new(Mode::Riding);
        let mut settings = Settings::default();
        let scratch = super::super::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut state,
            activity: &mut activity,
            settings: &mut settings,
            routes: &[],
            rides: &[],
            trips: &[],
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
            sensor_scan_hits: &[],
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    #[test]
    fn fixed_ring_dispatches_all_five_stations_in_order() {
        fn press_at(turn: i32) -> Transition {
            let mut scr = RideMenuScreen::new();
            run(&mut scr, Gesture::Turn(turn));
            run(&mut scr, Gesture::Press)
        }
        assert!(matches!(press_at(0), Transition::Push(Screen::RideWaypoints(_))));
        assert!(matches!(press_at(1), Transition::Push(Screen::SkipAhead(_))));
        assert!(matches!(press_at(2), Transition::Push(Screen::PoiMenu(_))));
        assert!(matches!(press_at(3), Transition::Push(Screen::RouteMenu(_))));
        assert!(matches!(press_at(-1), Transition::Push(Screen::Menu(_))));
    }

    #[test]
    fn ring_wrap_and_back_are_stable() {
        let mut scr = RideMenuScreen::new();
        run(&mut scr, Gesture::Turn(5));
        assert!(matches!(run(&mut scr, Gesture::Press), Transition::Push(Screen::RideWaypoints(_))));
        assert!(matches!(run(&mut RideMenuScreen::new(), Gesture::Back), Transition::Pop));
    }

    #[test]
    fn route_less_ring_dims_only_the_two_route_dependent_stations() {
        let route_less = CompassIcons::Ride { route_loaded: false };
        assert!(!route_less.enabled(0));
        assert!(!route_less.enabled(1));
        assert!((2..N_ITEMS).all(|i| route_less.enabled(i)));
        assert!((0..N_ITEMS).all(|i| CompassIcons::Ride { route_loaded: true }.enabled(i)));
    }

    #[test]
    fn ride_menu_uses_the_shared_needle_timer() {
        let mut scr = RideMenuScreen::new();
        run(&mut scr, Gesture::Turn(1));
        assert_eq!(scr.tick_timers(0).next_wake_ms, Some(16));
        let tick = scr.tick_timers(16);
        assert!(tick.changed);
        assert_eq!(tick.next_wake_ms, Some(16));
    }

    #[test]
    fn stub_children_only_pop_on_back() {
        assert!(matches!(stub_handle(Gesture::Back), Transition::Pop));
        for g in [Gesture::Turn(1), Gesture::Press, Gesture::Hold, Gesture::BackHold] {
            assert!(matches!(stub_handle(g), Transition::None));
        }
    }
}
