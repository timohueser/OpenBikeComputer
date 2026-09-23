//! Arrival at the end of the loaded route: when it happens, and when the rider has ridden on.
//!
//! Arrival is once per loaded route and per ride. After it the route is done: progress stays at the
//! route end, so every to-go figure reads 0, and a rider who rides on past the end is not off the
//! route. A new route, a new ride or new bytes for the route start it again.

use obc_map_scene::ground_dist_m;
use obc_route::RouteReader;

use super::RouteState;

/// Metres from the end, along the route and in a straight line to its last point, at which the
/// rider has arrived. Along the route is what keeps the start of a loop from counting as its end:
/// the matcher's forward bias holds progress near 0 there.
pub(crate) const ROUTE_ARRIVAL_M: u32 = 30;

/// Straight-line metres from the route's last point at which a rider who arrived has ridden on.
pub(crate) const RIDDEN_ON_M: f32 = 150.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Arrival {
    #[default]
    Riding,
    /// At the route's last point, `(lon, lat)` µdeg.
    Arrived { end: (i32, i32) },
    /// Arrived, then rode on past [`RIDDEN_ON_M`].
    RodeOn,
}

impl Arrival {
    /// The state after one fresh fix, `(lon, lat)` µdeg. While riding, `following` is the match of
    /// this fix.
    pub(super) fn on_fix(self, fix: (i32, i32), route: &RouteReader, following: &RouteState) -> Arrival {
        match self {
            Arrival::Riding => {
                let total = route.total_distance_m;
                if following.off_route || total.saturating_sub(following.progress_m) > ROUTE_ARRIVAL_M {
                    return self;
                }
                // One chunk decode, and only within the last metres of the route.
                match route.position_at(total).map(|end| (end.lon, end.lat)) {
                    Some(end) if ground_dist_m(fix, end) <= ROUTE_ARRIVAL_M as f32 => Arrival::Arrived { end },
                    _ => self,
                }
            }
            Arrival::Arrived { end } if ground_dist_m(fix, end) >= RIDDEN_ON_M => Arrival::RodeOn,
            _ => self,
        }
    }
}

#[cfg(test)]
mod tests {
    use obc_formats::io::SliceSource;
    use obc_ports::{Fix, InputClock, LocationSource, RideClock, Sensors};
    use obc_route::{RouteIndex, RouteReader};

    use super::Arrival;
    use crate::activity::Mode;
    use crate::device_core::{DerivedInputs, DerivedTargets, ExternalFacts, OutcomeSlots, PassClock, PassInputs};
    use crate::harness::support::{mount_store, quiet_pass, VecSink, EVERY_CAPABILITY};
    use crate::screen::{self, MapScreen, RouteSwapScreen, Screen, Transition, WarningFlags};
    use crate::{App, AppState, Gesture, RecorderIntent};

    /// A route through `points`, `(lon, lat)` in thousandths of a degree (about 111 m at 0°).
    fn route(points: &[(i32, i32)]) -> Vec<u8> {
        let mut gpx = String::from("<gpx><trk><trkseg>");
        for (lon, lat) in points {
            gpx += &format!("<trkpt lon=\"{}\" lat=\"{}\"/>", *lon as f64 / 1000.0, *lat as f64 / 1000.0);
        }
        gpx += "</trkseg></trk></gpx>";
        let mut sink = VecSink::default();
        obc_route::gpx_to_obcr(&SliceSource(gpx.as_bytes()), "Day 2 Ulrichen", &mut sink).unwrap();
        sink.0
    }

    /// Ten points north along lon 0: 1.1 km, ending at lat 10 000 µdeg.
    fn line() -> Vec<u8> {
        route(&(0..=10).map(|k| (0, k)).collect::<Vec<_>>())
    }

    struct Once(Option<Fix>);
    impl LocationSource for Once {
        fn poll(&mut self) -> Option<Fix> {
            self.0.take()
        }
    }

    /// One pass at `ms`, with a fresh fix at `(lon, lat)` µdeg when given.
    fn pass(app: &mut App, route: &RouteReader, ms: u32, fix: Option<(i32, i32)>, gestures: &[Gesture]) {
        let mut loc = Once(fix.map(|(lon, lat)| Fix::at(lat, lon)));
        let mut facts = ExternalFacts::NONE;
        app.run_pass(PassInputs {
            now: PassClock { ride: RideClock(ms), ui: InputClock(ms) },
            gestures,
            sensors: Sensors::new(&mut loc),
            route: Some(route),
            support: EVERY_CAPABILITY,
            outcomes: &mut OutcomeSlots::new(),
            facts: &mut facts,
            derived: DerivedInputs::NONE,
            targets: DerivedTargets::NONE,
        });
    }

    /// Ride through `fixes`, one pass each, then one more pass with no fix.
    fn ride(app: &mut App, route: &RouteReader, fixes: &[(i32, i32)]) {
        for &fix in fixes {
            let ms = app.ui.now_ms + 1_000;
            pass(app, route, ms, Some(fix), &[]);
        }
        let ms = app.ui.now_ms + 1_000;
        pass(app, route, ms, None, &[]);
    }

    fn press(app: &mut App, route: &RouteReader, gestures: &[Gesture]) {
        let ms = app.ui.now_ms + 1_000;
        pass(app, route, ms, None, gestures);
    }

    /// Recording on catalog route 0 of `routes`, on the Map.
    fn recording(routes: &[(obc_route::RouteSummary, u64)]) -> App {
        recording_trip(routes, &[])
    }

    fn recording_trip(routes: &[(obc_route::RouteSummary, u64)], trips: &[crate::trip::TripInput]) -> App {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let (summaries, ids): (Vec<_>, Vec<_>) = routes.iter().cloned().unzip();
        app.set_routes_with_ids(&summaries, &ids);
        app.set_trips(trips);
        mount_store(&mut app);
        app.activate_route(0);
        app.recorder.request(RecorderIntent::Start);
        quiet_pass(&mut app, 1);
        assert!(app.recorder.recording());
        app.activity.mode = Mode::Riding;
        screen::apply(&mut app.ui.stack, Transition::Root(Screen::Map(MapScreen::new())));
        app
    }

    fn arrival(app: &App) -> Arrival {
        app.navigator.route_state().arrival
    }

    fn view_up(app: &App) -> bool {
        matches!(app.top_screen(), Screen::Arrival(_))
    }

    #[test]
    fn arrival_at_the_end_of_a_route_holds_progress_there_and_finish_ride_saves() {
        let bytes = line();
        let src = SliceSource(&bytes);
        let index = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&index, &src);
        let mut app = recording(&[(route.summary(), 7)]);

        ride(&mut app, &route, &[(0, 0), (0, 4_000), (0, 8_000), (0, 9_500)]);
        assert_eq!(arrival(&app), Arrival::Riding, "55 m before the end");
        assert!(!view_up(&app));

        ride(&mut app, &route, &[(0, 9_800)]);
        assert!(matches!(arrival(&app), Arrival::Arrived { .. }), "22 m before the end");
        assert_eq!(app.navigator.route_state().progress_m, route.total_distance_m, "nothing to go");
        assert!(view_up(&app));

        press(&mut app, &route, &[Gesture::Press]);
        assert!(view_up(&app), "Finish ride needs the hold");
        press(&mut app, &route, &[Gesture::Hold]);
        assert!(matches!(app.top_screen(), Screen::Home(_)));
        assert_eq!(app.activity.mode, Mode::Idle);
    }

    #[test]
    fn a_loop_arrives_at_its_end_and_not_at_its_start() {
        let bytes = route(&[(0, 0), (0, 4), (4, 4), (4, 0), (0, 0)]);
        let src = SliceSource(&bytes);
        let index = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&index, &src);
        let mut app = recording(&[(route.summary(), 7)]);

        ride(&mut app, &route, &[(0, 100)]);
        assert_eq!(arrival(&app), Arrival::Riding, "11 m from the end point, at the start");
        ride(&mut app, &route, &[(0, 2_000), (0, 4_000), (2_000, 4_000), (4_000, 4_000), (4_000, 2_000), (4_000, 0)]);
        ride(&mut app, &route, &[(2_000, 0), (100, 0)]);
        assert!(matches!(arrival(&app), Arrival::Arrived { .. }));
        assert!(view_up(&app));
    }

    #[test]
    fn a_fix_near_the_end_but_off_the_route_does_not_arrive() {
        let bytes = line();
        let src = SliceSource(&bytes);
        let index = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&index, &src);
        let mut app = recording(&[(route.summary(), 7)]);

        // 28 m east of the end point: within the arrival radius, but off the route.
        ride(&mut app, &route, &[(0, 0), (0, 5_000), (0, 9_000), (250, 10_000)]);
        assert!(app.navigator.route_state().off_route);
        assert_eq!(arrival(&app), Arrival::Riding);
        ride(&mut app, &route, &[(0, 10_000)]);
        assert!(matches!(arrival(&app), Arrival::Arrived { .. }));
    }

    #[test]
    fn riding_on_150_m_closes_the_view_without_a_choice_and_it_never_comes_back() {
        let bytes = line();
        let src = SliceSource(&bytes);
        let index = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&index, &src);
        let mut app = recording(&[(route.summary(), 7)]);

        ride(&mut app, &route, &[(0, 0), (0, 5_000), (0, 9_900)]);
        assert!(view_up(&app));
        ride(&mut app, &route, &[(0, 11_200)]);
        assert!(view_up(&app), "133 m past the end");
        ride(&mut app, &route, &[(0, 11_400)]);
        assert_eq!(arrival(&app), Arrival::RodeOn);
        assert!(matches!(app.top_screen(), Screen::Map(_)), "closed, and the ride goes on");
        assert!(app.recorder.recording());
        assert_eq!(app.navigator.route_state().progress_m, route.total_distance_m);
        assert!(!app.navigator.route_state().off_route, "past the end is not off the route");

        ride(&mut app, &route, &[(0, 10_000)]);
        assert!(!view_up(&app), "once per loaded route");
    }

    #[test]
    fn keep_riding_closes_the_view_for_good() {
        let bytes = line();
        let src = SliceSource(&bytes);
        let index = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&index, &src);
        let mut app = recording(&[(route.summary(), 7)]);

        ride(&mut app, &route, &[(0, 0), (0, 5_000), (0, 9_900)]);
        press(&mut app, &route, &[Gesture::Step(1), Gesture::Press]);
        assert!(matches!(app.top_screen(), Screen::Map(_)));
        ride(&mut app, &route, &[(0, 10_000), (0, 10_300)]);
        assert!(!view_up(&app), "the rider is still at the end");
    }

    #[test]
    fn the_view_waits_for_a_warning_and_a_confirmation_to_close() {
        let bytes = line();
        let src = SliceSource(&bytes);
        let index = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&index, &src);
        let mut app = recording(&[(route.summary(), 7)]);

        app.on_warning(WarningFlags::NO_COMPASS);
        ride(&mut app, &route, &[(0, 0), (0, 5_000), (0, 9_900)]);
        assert!(matches!(app.top_screen(), Screen::Warning(_)), "a warning stays in front");
        press(&mut app, &route, &[Gesture::Back]);
        assert!(view_up(&app), "the view lands when the warning closes");

        let mut app = recording(&[(route.summary(), 7)]);
        screen::apply(&mut app.ui.stack, Transition::Push(Screen::RouteSwap(RouteSwapScreen::new(0))));
        ride(&mut app, &route, &[(0, 0), (0, 5_000), (0, 9_900)]);
        assert!(matches!(app.top_screen(), Screen::RouteSwap(_)), "a confirmation stays in front");
        press(&mut app, &route, &[Gesture::Back]);
        assert!(view_up(&app));
    }

    #[test]
    fn ride_on_loads_the_next_trip_day_and_keeps_recording() {
        let bytes = line();
        let src = SliceSource(&bytes);
        let index = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&index, &src);
        let stages = [7, 8];
        let trip = crate::trip::TripInput { id: 1, key: 42, name: "Alps", start_date: 0, stage_ids: &stages };
        let mut app = recording_trip(&[(route.summary(), 7), (route.summary(), 8)], &[trip]);

        ride(&mut app, &route, &[(0, 0), (0, 5_000), (0, 9_900)]);
        assert!(view_up(&app));
        press(&mut app, &route, &[Gesture::Step(1), Gesture::Press]);
        assert_eq!(app.active_route_index(), Some(1), "Day 2 is loaded");
        assert!(matches!(app.top_screen(), Screen::Map(_)));
        assert!(app.recorder.recording());
        assert_eq!(app.ride_stats().trip.map(|day| day.day_index()), Some(0), "the ride keeps its day");
    }
}
