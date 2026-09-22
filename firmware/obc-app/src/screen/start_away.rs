//! The Start-away prompt: START RIDE pressed with a fix more than [`START_AWAY_M`] from the route's
//! start. Ride to start plans the way there and rides it in front of the route; Join nearest follows
//! the route from its nearest point. Every distance on the page is a straight line from the fix at
//! the press, so the page shows at once, without planning.

use core::fmt::Write;

use obc_route::RouteMatch;

use super::vocab::chrome::{title_frame, TITLE_BAR_H};
use super::vocab::fmt::write_distance_split;
use super::vocab::rows::{draw_prompt, PromptOption};
use super::{Ctx, NavPlanningScreen, Prepare, Render, Screen, Transition};
use crate::input::Gesture;
use crate::navigator::NavigatorIntent;
use crate::settings::Units;
use crate::{DetourRequest, Msg};
use obc_render::{rect, text::Font, Surface};

/// Straight-line metres from the route start past which START RIDE asks how to begin.
pub(crate) const START_AWAY_M: u32 = 200;

/// Join nearest shows only when its point is this far along the route. Nearer the start it is the
/// same as Ride to start.
const JOIN_MIN_M: u32 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    RideToStart,
    JoinNearest,
    Cancel,
}

/// The route point nearest the fix, which needs the route geometry and so waits for `prepare`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Nearest {
    Pending,
    Found { dist_m: u32, along_m: u32 },
    Missing,
}

#[derive(Debug)]
pub struct StartAwayScreen {
    route: usize,
    /// The fix at the press, `(lon, lat)` µdeg.
    from: (i32, i32),
    start_m: u32,
    nearest: Nearest,
    /// Ride to start found no way, so only Join nearest and Cancel are left.
    no_route: bool,
    selected: Row,
}

impl StartAwayScreen {
    /// The prompt for START RIDE on catalog route `route`, or `None` when the rider can start as
    /// usual: no fix, or within [`START_AWAY_M`] of the start.
    pub(crate) fn ask(cx: &Ctx, route: usize) -> Option<Self> {
        let fix = cx.state.user_fix?;
        let summary = cx.routes.get(route)?;
        let from = (fix.lon, fix.lat);
        let start_m = obc_map_scene::ground_dist_m(from, (summary.start_lon, summary.start_lat)) as u32;
        (start_m > START_AWAY_M).then_some(StartAwayScreen {
            route,
            from,
            start_m,
            nearest: Nearest::Pending,
            no_route: false,
            selected: Row::RideToStart,
        })
    }

    /// The prompt with the plan mock's figures, for the copy-fit gate.
    #[cfg(test)]
    pub(crate) fn sample(no_route: bool) -> Self {
        let nearest = Nearest::Found { dist_m: 4_100, along_m: 12_000 };
        StartAwayScreen { route: 0, from: (0, 0), start_m: 3_200, nearest, no_route, selected: Row::RideToStart }
    }

    /// Ride to start found no way: leave Join nearest and Cancel, the cursor on the first.
    pub(crate) fn set_no_route(&mut self) {
        self.no_route = true;
        self.selected = self.rows()[0];
    }

    fn join(&self) -> Option<(u32, u32)> {
        match self.nearest {
            Nearest::Found { dist_m, along_m } if along_m >= JOIN_MIN_M => Some((dist_m, along_m)),
            _ => None,
        }
    }

    fn rows(&self) -> heapless::Vec<Row, 3> {
        let mut rows = heapless::Vec::new();
        if !self.no_route {
            let _ = rows.push(Row::RideToStart);
        }
        if self.join().is_some() {
            let _ = rows.push(Row::JoinNearest);
        }
        let _ = rows.push(Row::Cancel);
        rows
    }

    /// The cursor's row index. A row that went away puts the cursor on the first.
    fn cursor(&self, rows: &[Row]) -> usize {
        rows.iter().position(|&r| r == self.selected).unwrap_or(0)
    }

    /// Project the fix onto the whole route once the geometry is open.
    pub(crate) fn prepare(&mut self, px: &mut Prepare) {
        if self.nearest != Nearest::Pending || px.active_route != Some(self.route) {
            return;
        }
        let Some(route) = px.route else { return };
        self.nearest = match RouteMatch::nearest(self.from.0, self.from.1, route) {
            Some(m) => Nearest::Found { dist_m: m.dist_m, along_m: m.progress_m },
            None => Nearest::Missing,
        };
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let rows = self.rows();
        let i = self.cursor(&rows);
        match g {
            Gesture::Step(n) => {
                self.selected = rows[super::vocab::list::step_selection(i, n, rows.len())];
                Transition::None
            }
            Gesture::Press => match rows[i] {
                Row::RideToStart => {
                    cx.navigator
                        .admit_intent(NavigatorIntent::PlanDetour(DetourRequest::approach(self.route, self.from)));
                    Transition::Push(Screen::NavPlanning(NavPlanningScreen::approach()))
                }
                Row::JoinNearest => {
                    if let Some((_, along_m)) = self.join() {
                        cx.navigator.join_at(along_m);
                    }
                    super::start_ride(cx, self.route)
                }
                Row::Cancel => Transition::Pop,
            },
            Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        let name = rx.routes.get(self.route).map_or("", |r| r.name.as_str());
        let title = rx.marquee.fit(name, w - 28, Font::Body, Some(rect(0, 0, w, TITLE_BAR_H)));
        title_frame(cv, w, h, &title, "");

        let units = rx.settings.units;
        let mut question: heapless::String<64> = heapless::String::new();
        if self.no_route {
            let _ = question.push_str(rx.t(Msg::StartAwayNoRoute));
        } else {
            let _ = question.push_str(rx.t(Msg::StartAwayYouAre));
            push_distance(&mut question, self.start_m, units);
            let _ = question.push_str(rx.t(Msg::StartAwayFromStart));
        }

        let mut to_start: heapless::String<32> = heapless::String::new();
        push_distance(&mut to_start, self.start_m, units);
        let _ = to_start.push_str(rx.t(Msg::StartAwayThenRoute));
        let mut to_join: heapless::String<32> = heapless::String::new();
        if let Some((dist_m, along_m)) = self.join() {
            push_distance(&mut to_join, dist_m, units);
            let _ = to_join.push_str(rx.t(Msg::StartAwayAwayAt));
            let unit = if units.is_imperial() { "mi" } else { "km" };
            let _ = write!(to_join, "{unit} {}", (units.dist(along_m as f32 / 1000.0) + 0.5) as u32);
        }

        let rows = self.rows();
        let mut options: heapless::Vec<PromptOption, 3> = heapless::Vec::new();
        for row in &rows {
            let option = match row {
                Row::RideToStart => PromptOption { label: rx.t(Msg::StartAwayRideToStart), hint: Some(&to_start) },
                Row::JoinNearest => PromptOption { label: rx.t(Msg::StartAwayJoinNearest), hint: Some(&to_join) },
                Row::Cancel => PromptOption { label: rx.t(Msg::StartAwayCancel), hint: None },
            };
            let _ = options.push(option);
        }
        draw_prompt(cv, w, &question, &options, self.cursor(&rows));
    }
}

/// `3.2 km` or `400 m`: whole metres below a kilometre, one decimal above.
fn push_distance<const N: usize>(s: &mut heapless::String<N>, m: u32, units: Units) {
    let mut value: heapless::String<8> = heapless::String::new();
    let unit = write_distance_split(&mut value, m, units);
    let _ = write!(s, "{value} {unit}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::test_ctx;
    use crate::settings::Settings;
    use crate::{Activity, AppState};
    use obc_ports::Fix;

    fn route() -> obc_route::RouteSummary {
        obc_route::RouteSummary {
            name: heapless::String::try_from("Day 2").unwrap(),
            distance_km: 20,
            climb_m: 0,
            bbox: obc_map_scene::BBox { min_lon: 0, min_lat: 0, max_lon: 0, max_lat: 0 },
            start_lon: 0,
            start_lat: 0,
        }
    }

    /// The prompt asks only with a fix beyond the threshold; the rows follow what is known.
    #[test]
    fn the_prompt_asks_only_away_from_the_start_and_offers_what_it_knows() {
        let routes = [route()];
        let mut settings = Settings::default();
        let mut activity = Activity::new(crate::Mode::Idle);
        // 0.001° of latitude is about 111 m, so 0.002° is past the threshold and 0.001° is not.
        for (lat, asks) in [(1_000, false), (2_000, true)] {
            let mut state = AppState::new(0, 0, 1.0);
            state.user_fix = Some(Fix::at(lat, 0));
            let cx = Ctx { routes: &routes, ..test_ctx(&mut state, &mut activity, &mut settings) };
            assert_eq!(StartAwayScreen::ask(&cx, 0).is_some(), asks, "fix {lat} µdeg north of the start");
        }
        let mut state = AppState::new(0, 0, 1.0);
        let cx = Ctx { routes: &routes, ..test_ctx(&mut state, &mut activity, &mut settings) };
        assert!(StartAwayScreen::ask(&cx, 0).is_none(), "without a fix START RIDE starts as usual");

        let mut state = AppState::new(0, 0, 1.0);
        state.user_fix = Some(Fix::at(30_000, 0));
        let cx = Ctx { routes: &routes, ..test_ctx(&mut state, &mut activity, &mut settings) };
        let mut prompt = StartAwayScreen::ask(&cx, 0).unwrap();
        assert_eq!(prompt.rows().as_slice(), [Row::RideToStart, Row::Cancel], "no Join row before the projection");
        prompt.nearest = Nearest::Found { dist_m: 400, along_m: JOIN_MIN_M - 1 };
        assert_eq!(prompt.rows().as_slice(), [Row::RideToStart, Row::Cancel], "a join near the start is Ride to start");
        prompt.nearest = Nearest::Found { dist_m: 400, along_m: 12_000 };
        assert_eq!(prompt.rows().as_slice(), [Row::RideToStart, Row::JoinNearest, Row::Cancel]);
        prompt.set_no_route();
        assert_eq!(prompt.rows().as_slice(), [Row::JoinNearest, Row::Cancel]);
        assert_eq!(prompt.selected, Row::JoinNearest, "the cursor lands on Join nearest");
    }
}
