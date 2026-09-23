//! The start card, opened by a press on the browse Map. It stacks the hero sprite of the current
//! bike over its name, a GPS and battery line, and the rows: the bike, Start ride, the active trip's
//! next day when there is one, and Back.
//!
//! The bike row opens the bike type editor, whose sheet draws the staged bike over the hero. Start
//! ride begins a tracking session with no route. The day row opens the day's route detail, so every
//! routed start goes through a route detail. Starting is a press, not a hold, because it is
//! reversible: Discard on the Paused page throws the new session away.

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::settings::BikeType;
use crate::Msg;

use super::context_drawer::{ContextDrawerScreen, ContextValue};
use super::settings::bike_icons;
use super::vocab::chrome::title_frame;
use super::{palette, Ctx, Render, Screen, Transition};

/// Top of the hero bike, just under the title bar, and its art-pixel scale.
const HERO_TOP: i32 = 32;
const HERO_SCALE: i32 = 2;
/// The hero's box, which the editor sheet repaints with the staged bike.
const HERO_H: i32 = 58;
/// The bike row: the name on its cursor band.
const BIKE_TOP: i32 = 92;
const BIKE_H: i32 = 29;
/// The GPS and battery line.
const STATUS_TOP: i32 = 124;
/// The first option row, and each row's height and the space under it.
const ROWS_TOP: i32 = 150;
const ROW_H: i32 = 35;
const DAY_H: i32 = 55;
const ROW_GAP: i32 = 9;
/// The cursor band's side insets, and the row text's left edge.
const BAND_X: i32 = 6;
const TEXT_X: i32 = 14;
/// Where a row's text sits in its band.
const TEXT_DY: i32 = 5;
const DAY_LINE2_DY: i32 = 28;

/// The rows the cursor walks, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Bike,
    Start,
    /// The active trip's next day. It is in the list only while there is one.
    Day,
    Back,
}

const ROWS: [Row; 4] = [Row::Bike, Row::Start, Row::Day, Row::Back];

#[derive(Debug)]
pub struct RideStartScreen {
    selected: Row,
}

impl Default for RideStartScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl RideStartScreen {
    pub fn new() -> Self {
        RideStartScreen { selected: Row::Start }
    }

    fn rows(day: bool) -> impl Iterator<Item = Row> {
        ROWS.into_iter().filter(move |&row| day || row != Row::Day)
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let next = crate::trip::next_trip_day(cx.trips, cx.trip_progress);
        match g {
            Gesture::Step(n) => {
                let rows: heapless::Vec<Row, 4> = Self::rows(next.is_some()).collect();
                let at = rows.iter().position(|&row| row == self.selected).unwrap_or(1);
                self.selected = rows[super::vocab::list::step_selection(at, n, rows.len())];
                Transition::None
            }
            Gesture::Press => match self.selected {
                Row::Bike => Transition::Push(Screen::ContextDrawer(
                    ContextDrawerScreen::editor(ContextValue::BikeProfile, Msg::RideBikeType, &cx.context_facts())
                        .over_hero(),
                )),
                Row::Start => super::start_ride_routeless(cx),
                Row::Day => match next {
                    Some((_, _, route)) => {
                        let route = usize::from(route);
                        let prev = cx.navigator.replace_active_route(route);
                        Transition::Push(Screen::RouteOverview(super::RouteOverviewScreen::new(route, prev)))
                    }
                    None => Transition::None,
                },
                Row::Back => Transition::Pop,
            },
            Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let w = rx.w;
        title_frame(cv, w, rx.h, rx.t(Msg::RideStartTitle), "");

        let bike = rx.settings.bike_type;
        draw_hero(cv, w, bike);
        let on_bike = self.selected == Row::Bike;
        if on_bike {
            cv.round(rect(BAND_X, BIKE_TOP, w - 2 * BAND_X, BIKE_H), 5, AMBER);
        }
        let name = crate::settings::bike_type_name(bike, rx.settings.language);
        let ink = if on_bike { INK } else { SUBTEXT };
        cv.text(name, Point::new(w / 2, BIKE_TOP + 4), Font::Caption, TextAlign::Center, ink);

        let mut status: heapless::String<48> = heapless::String::new();
        let gps = if rx.no_fix { rx.t(Msg::RideStartSearching) } else { rx.t(Msg::RideStartFix) };
        let pct = rx.state.device.battery_pct;
        let _ = write!(status, "{} {gps} · {} {pct}%", rx.t(Msg::RideStartGps), rx.t(Msg::RideStartBattery));
        // A long translation drops the battery's name and keeps its figure.
        if text_width(&status, Font::Caption) as i32 > w - 2 * BAND_X {
            status.clear();
            let _ = write!(status, "{} {gps} · {pct}%", rx.t(Msg::RideStartGps));
        }
        cv.text(&status, Point::new(w / 2, STATUS_TOP), Font::Caption, TextAlign::Center, SUBTEXT);

        let next = crate::trip::next_trip_day(rx.trips, rx.trip_progress);
        let mut top = ROWS_TOP;
        for row in Self::rows(next.is_some()).skip(1) {
            let h = if row == Row::Day { DAY_H } else { ROW_H };
            if row == self.selected {
                cv.round(rect(BAND_X, top, w - 2 * BAND_X, h), 5, AMBER);
            }
            let text_top = top + TEXT_DY;
            match (row, next.and_then(|(_, _, route)| rx.routes.get(usize::from(route)))) {
                (Row::Day, Some(route)) => {
                    let name = rx.marquee.fit(&route.name, w - TEXT_X - BAND_X, Font::Label, None);
                    cv.text(&name, Point::new(TEXT_X, text_top - 2), Font::Label, TextAlign::Left, INK);
                    let mut line2: heapless::String<40> = heapless::String::new();
                    let _ = write!(line2, "{} · {} km", rx.t(Msg::RideStartShowRoute), route.distance_km);
                    let line2_top = text_top - 2 + DAY_LINE2_DY - 2;
                    cv.text(&line2, Point::new(TEXT_X, line2_top), Font::Caption, TextAlign::Left, SUBTEXT);
                }
                (Row::Start, _) => {
                    cv.text(
                        rx.t(Msg::RideStartStartRide),
                        Point::new(TEXT_X, text_top),
                        Font::Label,
                        TextAlign::Left,
                        INK,
                    );
                }
                (Row::Back, _) => {
                    cv.text(
                        rx.t(Msg::RideStartBack),
                        Point::new(TEXT_X, text_top - 1),
                        Font::Label,
                        TextAlign::Left,
                        INK,
                    );
                }
                _ => {}
            }
            top += h + ROW_GAP;
        }
    }
}

/// The hero bike in its box. The bike type editor over this card draws it too, with the staged type.
pub(crate) fn draw_hero(cv: &mut impl Surface, w: i32, bike: BikeType) {
    bike_icons::draw(cv, bike_icons::sprite(bike), w / 2, HERO_TOP, HERO_SCALE, bike_icons::color(bike));
}

/// The hero's box inside the card's frame, for a sheet that repaints it.
pub(crate) fn hero_box(w: i32) -> embedded_graphics::primitives::Rectangle {
    rect(BAND_X, HERO_TOP, w - 2 * BAND_X, HERO_H)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::screen::test_ctx;
    use crate::screen::Screen;
    use crate::{AppState, Settings};

    fn run(
        scr: &mut RideStartScreen,
        st: &mut AppState,
        act: &mut Activity,
        rec: &mut crate::RecorderMachine,
        navigator: &mut crate::navigator::NavigatorMachine,
        g: Gesture,
    ) -> Transition {
        let mut settings = Settings::default();
        let mut cx = Ctx { recorder: rec, navigator, ..test_ctx(st, act, &mut settings) };
        scr.handle(g, &mut cx)
    }

    #[test]
    fn start_begins_a_route_less_session_and_roots_the_stack() {
        let mut rec = crate::RecorderMachine::new();
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut navigator = crate::navigator::NavigatorMachine::new();
        let mut scr = RideStartScreen::new(); // the selection starts on "Start ride"
        let t = run(&mut scr, &mut st, &mut act, &mut rec, &mut navigator, Gesture::Press);
        assert!(matches!(t, Transition::Root(Screen::Map(_))), "start roots to [Home, Map]");
        assert_eq!(act.mode, Mode::Riding, "start enters riding mode");
        assert_eq!(rec.test_take_intent(), Some(crate::RecorderIntent::Start), "the ride is named to Recorder");
        assert_eq!(navigator.route_state().active_route, None, "no route is attached — this is a route-less ride");
    }

    #[test]
    fn back_pops_without_starting() {
        let mut rec = crate::RecorderMachine::new();
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut navigator = crate::navigator::NavigatorMachine::new();
        let mut scr = RideStartScreen::new();
        run(&mut scr, &mut st, &mut act, &mut rec, &mut navigator, Gesture::Step(1)); // highlight "Back"
        let t = run(&mut scr, &mut st, &mut act, &mut rec, &mut navigator, Gesture::Press);
        assert!(matches!(t, Transition::Pop), "Back pops");
        assert!(!rec.recording(), "nothing started");

        // The back gesture pops from any selection, too.
        let mut scr = RideStartScreen::new();
        let t = run(&mut scr, &mut st, &mut act, &mut rec, &mut navigator, Gesture::Back);
        assert!(matches!(t, Transition::Pop));
        assert!(!rec.recording());
    }

    /// The day row sits between Start ride and Back while the active trip has a next day. A press
    /// on it opens that day's route detail with the day loaded; a press on the bike opens the
    /// bike type editor over the hero.
    #[test]
    fn the_day_row_opens_the_next_day_and_the_bike_row_opens_the_editor() {
        use crate::trip::{TripInput, TripProgress, TripSummary};
        let route = || crate::route::RouteSummary {
            name: heapless::String::new(),
            distance_km: 61,
            climb_m: 900,
            bbox: obc_map_scene::BBox { min_lon: 0, min_lat: 0, max_lon: 1, max_lat: 1 },
            start_lon: 0,
            start_lat: 0,
        };
        let routes = [route(), route()];
        let input = TripInput { id: 1, key: 9, name: "Alps", start_date: 0, stage_ids: &[70, 80] };
        let trips = [TripSummary::resolve(&input, &routes, &[70, 80])];
        let day1 = TripProgress {
            key: 9,
            day: 0,
            day_route: crate::trip::RouteVersion { id: 70, revision: 0 },
            metres: 5_000,
            last_finished: Some(0),
            dates: [0; obc_route::MAX_TRIP_DAYS],
        };
        let (mut st, mut act, mut settings) =
            (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle), Settings::default());
        let mut navigator = crate::navigator::NavigatorMachine::new();
        let records = [day1];
        let mut cx = Ctx {
            navigator: &mut navigator,
            routes: &routes,
            trips: &trips,
            trip_progress: &records,
            ..test_ctx(&mut st, &mut act, &mut settings)
        };
        let mut scr = RideStartScreen::new();
        scr.handle(Gesture::Step(1), &mut cx);
        assert!(matches!(scr.handle(Gesture::Press, &mut cx), Transition::Push(Screen::RouteOverview(_))));
        assert_eq!(cx.navigator.route_state().active_route, Some(1), "Day 2 is loaded under its detail");
        scr.handle(Gesture::Step(-2), &mut cx);
        let Transition::Push(Screen::ContextDrawer(editor)) = scr.handle(Gesture::Press, &mut cx) else {
            panic!("the bike row opens the editor");
        };
        assert!(editor.draws_hero());

        let mut cx = Ctx { trips: &trips, ..test_ctx(&mut st, &mut act, &mut settings) };
        let mut scr = RideStartScreen::new();
        scr.handle(Gesture::Step(1), &mut cx);
        assert!(matches!(scr.handle(Gesture::Press, &mut cx), Transition::Pop), "no progress, no day row");
    }
}
