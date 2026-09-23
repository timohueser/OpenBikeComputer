//! Arrival at the end of the loaded route: when it happens, and when the rider has ridden on.
//!
//! Arrival is once per loaded route and per ride. After it the route is done: progress stays at the
//! route end, so every to-go figure reads 0, and a rider who rides on past the end is not off the
//! route. A new route, a new ride or new bytes for the route start it again.

use obc_map_scene::ground_dist_m;
use obc_route::RouteReader;

use super::RouteState;

/// Metres before the end, along the route, at which an on-route rider has arrived. Along the route
/// is what keeps the start of a loop from counting as its end: the matcher's forward bias holds
/// progress near 0 there.
pub(crate) const ROUTE_ARRIVAL_M: u32 = 30;

/// Straight-line metres from the route's last point at which a fix arrives whatever the matcher
/// says. Sparse fixes can step over the last [`ROUTE_ARRIVAL_M`], and a fix past the end is off the
/// route: 10 s between fixes at 25 km/h is 69 m, and the GPS error adds about 15 m.
pub(crate) const END_RADIUS_M: f32 = 85.0;

/// The last matched progress must be this close to the end, and past half the route, for
/// [`END_RADIUS_M`] to count. So the start of a short loop or an out-and-back does not arrive.
const NEAR_END_M: u32 = 1_000;

/// Straight-line metres from the route's last point at which a rider who arrived has ridden on.
pub(crate) const RIDDEN_ON_M: f32 = 150.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Arrival {
    #[default]
    Riding,
    /// The last matched progress is near the end, whose last point, `(lon, lat)` µdeg, is decoded
    /// once.
    Near { end: (i32, i32) },
    /// At the route's last point.
    Arrived { end: (i32, i32) },
    /// Arrived, then rode on past [`RIDDEN_ON_M`].
    RodeOn,
}

impl Arrival {
    /// The state after one fresh fix, `(lon, lat)` µdeg. Before arrival, `following` is the match
    /// of this fix; its progress is the last matched one while the fix is off the route.
    pub(super) fn on_fix(self, fix: (i32, i32), route: &RouteReader, following: &RouteState) -> Arrival {
        let total = route.total_distance_m;
        let to_go = total.saturating_sub(following.progress_m);
        let end = match self {
            Arrival::Arrived { end } if ground_dist_m(fix, end) >= RIDDEN_ON_M => return Arrival::RodeOn,
            Arrival::Arrived { .. } | Arrival::RodeOn => return self,
            _ if to_go > NEAR_END_M.min(total / 2) => return self,
            Arrival::Near { end } => end,
            Arrival::Riding => match route.position_at(total) {
                Some(end) => (end.lon, end.lat),
                None => return self,
            },
        };
        let on_end = !following.off_route && to_go <= ROUTE_ARRIVAL_M;
        if on_end || ground_dist_m(fix, end) <= END_RADIUS_M {
            Arrival::Arrived { end }
        } else {
            Arrival::Near { end }
        }
    }

    /// Arrived at the end, whether or not the rider rode on after.
    pub(crate) fn arrived(self) -> bool {
        matches!(self, Arrival::Arrived { .. } | Arrival::RodeOn)
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
    use crate::screen::{self, ArrivalView, MapScreen, RouteSwapScreen, Screen, Transition, WarningFlags};
    use crate::trip::TripInput;
    use crate::{App, AppState, Gesture, RecorderIntent};

    /// A route through `points`, `(lon, lat)` in thousandths of a degree (about 111 m at 0°).
    fn route_named(name: &str, points: &[(i32, i32)]) -> Vec<u8> {
        route_udeg(name, &points.iter().map(|&(lon, lat)| (lon * 1_000, lat * 1_000)).collect::<Vec<_>>())
    }

    /// A route through `points`, `(lon, lat)` in µdeg.
    fn route_udeg(name: &str, points: &[(i32, i32)]) -> Vec<u8> {
        let mut gpx = String::from("<gpx><trk><trkseg>");
        for (lon, lat) in points {
            gpx += &format!("<trkpt lon=\"{:.6}\" lat=\"{:.6}\"/>", *lon as f64 / 1e6, *lat as f64 / 1e6);
        }
        gpx += "</trkseg></trk></gpx>";
        let mut sink = VecSink::default();
        obc_route::gpx_to_obcr(&SliceSource(gpx.as_bytes()), name, &mut sink).unwrap();
        sink.0
    }

    fn route(points: &[(i32, i32)]) -> Vec<u8> {
        route_named("Day 2 Ulrichen", points)
    }

    /// Ten points north along lon 0: 1.1 km, ending at lat 10 000 µdeg.
    fn line() -> Vec<u8> {
        route(&(0..=10).map(|k| (0, k)).collect::<Vec<_>>())
    }

    /// The line ridden back south: it starts where [`line`] ends.
    fn back() -> Vec<u8> {
        route(&(0..=10).rev().map(|k| (0, k)).collect::<Vec<_>>())
    }

    struct Once(Option<Fix>);
    impl LocationSource for Once {
        fn poll(&mut self) -> Option<Fix> {
            self.0.take()
        }
    }

    std::thread_local! {
        /// The executor's answers to the last pass, delivered on the next one.
        static OUTCOMES: core::cell::RefCell<OutcomeSlots> = const { core::cell::RefCell::new(OutcomeSlots::new()) };
    }

    /// One pass at `ms`, with a fresh fix at `(lon, lat)` µdeg when given. It answers the store work
    /// the pass asks for, as an executor would, and returns a trip progress record it writes.
    fn pass(
        app: &mut App,
        route: &RouteReader,
        ms: u32,
        fix: Option<(i32, i32)>,
        gestures: &[Gesture],
    ) -> Option<crate::trip::TripProgress> {
        use crate::catalog_state::{CatalogEffect, CatalogOutcome};
        use crate::device_core::{Revision, StoreIdentity, StoreRevision};
        use crate::metadata::{MetadataEffect, MetadataOutcome};
        use crate::recorder::{CheckpointStatus, RecorderEffect, RecorderOutcome};

        let scope = StoreRevision { store: StoreIdentity::new(1), revision: Revision::new(1) };
        let mut loc = Once(fix.map(|(lon, lat)| Fix::at(lat, lon)));
        let mut facts = ExternalFacts::NONE;
        facts.note_store_revision(scope);
        OUTCOMES.with_borrow_mut(|outcomes| {
            let mut plan = app.run_pass(PassInputs {
                now: PassClock { ride: RideClock(ms), ui: InputClock(ms) },
                gestures,
                sensors: Sensors::new(&mut loc),
                route: Some(route),
                support: EVERY_CAPABILITY,
                outcomes,
                facts: &mut facts,
                derived: DerivedInputs::NONE,
                targets: DerivedTargets::NONE,
            });
            if let Some(CatalogEffect::ReadCatalog { token }) = plan.effects.catalog.take() {
                let _ = outcomes.catalog.try_put(CatalogOutcome::CatalogRead { token, scope: Some(scope) });
            }
            let answer = match plan.effects.recorder.take() {
                Some(RecorderEffect::Append { token, samples }) => Some(RecorderOutcome::Appended { token, samples }),
                Some(RecorderEffect::Checkpoint { token }) => {
                    Some(RecorderOutcome::Checkpointed { token, status: CheckpointStatus::Durable })
                }
                Some(RecorderEffect::Finalize { token }) => Some(RecorderOutcome::Finalized { token, ride: 77 }),
                _ => None,
            };
            if let Some(answer) = answer {
                let _ = outcomes.recorder.try_put(answer);
            }
            let Some(MetadataEffect::WriteProgress { token, .. }) = plan.effects.metadata.take() else { return None };
            let _ = outcomes.metadata.try_put(MetadataOutcome::ProgressWritten { token });
            app.trip_progress_payload(token).cloned()
        })
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
        recording_on(routes, &[], 0, |_| {})
    }

    /// Recording on catalog route `first`, with `trips`; `prepare` runs before the ride starts.
    fn recording_on(
        routes: &[(obc_route::RouteSummary, u64)],
        trips: &[TripInput],
        first: usize,
        prepare: impl FnOnce(&mut App),
    ) -> App {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let (summaries, ids): (Vec<_>, Vec<_>) = routes.iter().cloned().unzip();
        app.set_routes_with_ids(&summaries, &ids);
        app.set_trips(trips);
        mount_store(&mut app);
        let store = crate::device_core::StoreIdentity::new(1);
        let scope = crate::device_core::StoreRevision { store, revision: crate::device_core::Revision::new(1) };
        app.catalogs.loaded_scope = Some(scope);
        app.activate_route(first);
        prepare(&mut app);
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

    fn view(app: &App) -> Option<ArrivalView> {
        match app.top_screen() {
            Screen::Arrival(view) => Some(view.view()),
            _ => None,
        }
    }

    fn view_up(app: &App) -> bool {
        view(app).is_some()
    }

    #[test]
    fn arrival_at_the_end_of_a_route_holds_progress_there_and_finish_ride_saves() {
        let bytes = line();
        let src = SliceSource(&bytes);
        let index = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&index, &src);
        let mut app = recording(&[(route.summary(), 7)]);

        ride(&mut app, &route, &[(0, 0), (0, 4_000), (0, 8_000), (0, 9_000)]);
        assert!(!arrival(&app).arrived(), "111 m before the end");
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
    fn sparse_fixes_that_step_over_the_end_arrive() {
        let bytes = line();
        let src = SliceSource(&bytes);
        let index = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&index, &src);
        let mut app = recording(&[(route.summary(), 7)]);

        // 10 s apart at 25 km/h: 122 m before the end, then 70 m past it and off the route.
        ride(&mut app, &route, &[(0, 0), (0, 5_000), (0, 8_900)]);
        assert!(!arrival(&app).arrived());
        ride(&mut app, &route, &[(0, 10_630)]);
        assert!(matches!(arrival(&app), Arrival::Arrived { .. }));
        assert!(!app.navigator.route_state().off_route);
        assert!(view_up(&app));
    }

    #[test]
    fn a_loop_and_an_out_and_back_arrive_at_their_end_and_not_at_their_start() {
        let paths: [&[(i32, i32)]; 2] = [&[(0, 0), (0, 4), (4, 4), (4, 0), (0, 0)], &[(0, 0), (0, 4), (0, 0)]];
        let rides: [&[(i32, i32)]; 2] = [
            &[(0, 2_000), (0, 4_000), (2_000, 4_000), (4_000, 4_000), (4_000, 2_000), (4_000, 0), (2_000, 0), (100, 0)],
            &[(0, 2_000), (0, 4_000), (0, 3_000), (0, 2_000), (0, 1_000), (0, 100)],
        ];
        for (path, fixes) in paths.into_iter().zip(rides) {
            let bytes = route(path);
            let src = SliceSource(&bytes);
            let index = RouteIndex::read(&src).unwrap();
            let route = RouteReader::new(&index, &src);
            let mut app = recording(&[(route.summary(), 7)]);

            ride(&mut app, &route, &[(0, 100)]);
            assert!(!arrival(&app).arrived(), "11 m from the end point, at the start");
            ride(&mut app, &route, fixes);
            assert!(matches!(arrival(&app), Arrival::Arrived { .. }), "{path:?}");
            assert!(view_up(&app));
        }
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
    fn keep_riding_closes_the_view_for_good_across_a_pause() {
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

        press(&mut app, &route, &[Gesture::Press]);
        assert!(matches!(app.top_screen(), Screen::RideControl(_)), "paused");
        press(&mut app, &route, &[Gesture::Press]);
        assert!(matches!(app.top_screen(), Screen::Map(_)), "resumed");
        ride(&mut app, &route, &[(0, 10_000)]);
        assert!(!view_up(&app), "a pause does not bring the view back");
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
        let back = back();
        let back_src = SliceSource(&back);
        let back_index = RouteIndex::read(&back_src).unwrap();
        let back_route = RouteReader::new(&back_index, &back_src);
        let trip = TripInput { id: 1, key: 42, name: "Alps", start_date: 0, stage_ids: &[7, 8] };
        let mut app = recording_on(&[(route.summary(), 7), (back_route.summary(), 8)], &[trip], 0, |_| {});

        ride(&mut app, &route, &[(0, 0), (0, 5_000), (0, 9_900)]);
        assert_eq!(view(&app), Some(ArrivalView { route: 0, day: Some(0), next: Some(1) }));
        press(&mut app, &route, &[Gesture::Step(1), Gesture::Press]);
        assert_eq!(app.active_route_index(), Some(1), "Day 2 is loaded");
        assert!(matches!(app.top_screen(), Screen::Map(_)));
        assert!(app.recorder.recording());
        assert_eq!(app.ride_stats().trip.map(|day| day.day_index()), Some(0), "the ride keeps its day");
    }

    /// Run passes until the store has saved the ride, and return the trip progress it writes.
    fn save(app: &mut App, route: &RouteReader) -> crate::trip::TripProgress {
        (0..40)
            .find_map(|_| {
                let ms = app.ui.now_ms + 1_000;
                pass(app, route, ms, None, &[])
            })
            .expect("the Finish writes the trip progress")
    }

    #[test]
    fn finish_at_the_end_of_the_day_ride_on_reached_finishes_that_day() {
        let bytes = line();
        let src = SliceSource(&bytes);
        let index = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&index, &src);
        // Day 1 rides north, Day 2 rides back south, Day 3 north again.
        let back = back();
        let back_src = SliceSource(&back);
        let back_index = RouteIndex::read(&back_src).unwrap();
        let back_route = RouteReader::new(&back_index, &back_src);
        let trip = TripInput { id: 1, key: 42, name: "Alps", start_date: 0, stage_ids: &[7, 8, 9] };
        let routes = [(route.summary(), 7), (back_route.summary(), 8), (route.summary(), 9)];
        let mut app = recording_on(&routes, &[trip], 0, |_| {});

        ride(&mut app, &route, &[(0, 0), (0, 5_000), (0, 9_900)]);
        press(&mut app, &route, &[Gesture::Step(1), Gesture::Press]);
        assert_eq!(app.active_route_index(), Some(1), "Day 2 is loaded");
        ride(&mut app, &back_route, &[(0, 10_000), (0, 5_000), (0, 100)]);
        assert_eq!(view(&app), Some(ArrivalView { route: 1, day: Some(1), next: Some(2) }));
        press(&mut app, &back_route, &[Gesture::Hold]);

        let written = save(&mut app, &back_route);
        assert_eq!((written.day, written.day_route.id, written.last_finished), (1, 8, Some(1)), "Day 2 is done");
        assert_eq!(written.metres, back_route.total_distance_m);
        let trip = &app.trips()[0];
        assert_eq!(trip.next_day(Some(&written)), Some(2), "Day 3 is next");
    }

    #[test]
    fn finish_from_the_paused_page_records_where_the_rider_stopped() {
        let bytes = line();
        let src = SliceSource(&bytes);
        let index = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&index, &src);
        let trip = TripInput { id: 1, key: 42, name: "Alps", start_date: 0, stage_ids: &[7, 8] };
        let mut app = recording_on(&[(route.summary(), 7), (route.summary(), 8)], &[trip], 0, |_| {});

        ride(&mut app, &route, &[(0, 0), (0, 2_500), (0, 5_000)]);
        let stopped = app.progress_m();
        assert!(stopped > 500 && !arrival(&app).arrived());
        press(&mut app, &route, &[Gesture::Press, Gesture::Step(1)]);
        assert!(matches!(app.top_screen(), Screen::RideControl(_)), "paused, the cursor on Finish");
        press(&mut app, &route, &[Gesture::Hold]);

        let written = save(&mut app, &route);
        assert_eq!((written.day, written.day_route.id, written.metres), (0, 7, stopped), "not 0 m");
        assert_eq!(written.last_finished, Some(0));
    }

    #[test]
    fn ride_on_is_offered_up_to_a_transfer() {
        use crate::trip::TRANSFER_MIN_M;
        let bytes = line();
        let src = SliceSource(&bytes);
        let index = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&index, &src);
        let end = route.position_at(route.total_distance_m).map(|end| (end.lon, end.lat)).unwrap();
        // The next day starts due north of where this day ends, 200 m and 201 m away.
        let north = |m: f32| (0..).find(|&k| obc_map_scene::ground_dist_m(end, (end.0, end.1 + k)) > m).unwrap();
        for (gap, offered) in [(north(TRANSFER_MIN_M as f32) - 1, true), (north(TRANSFER_MIN_M as f32 + 1.0), false)] {
            let start = end.1 + gap;
            let next = route_udeg("Day 2 Brig", &[(end.0, start), (end.0, start + 5_000)]);
            let next_src = SliceSource(&next);
            let next_index = RouteIndex::read(&next_src).unwrap();
            let next_route = RouteReader::new(&next_index, &next_src);
            let trip = TripInput { id: 1, key: 42, name: "Alps", start_date: 0, stage_ids: &[7, 8] };
            let mut app = recording_on(&[(route.summary(), 7), (next_route.summary(), 8)], &[trip], 0, |_| {});

            ride(&mut app, &route, &[(0, 0), (0, 5_000), (0, 9_900)]);
            let next = offered.then_some(1);
            assert_eq!(view(&app), Some(ArrivalView { route: 0, day: Some(0), next }), "{gap} µdeg");
            press(&mut app, &route, &[Gesture::Step(1), Gesture::Press]);
            let loaded = if offered { 1 } else { 0 };
            assert_eq!(app.active_route_index(), Some(loaded), "the second row is Ride on only below a transfer");
        }
    }

    #[test]
    fn a_derived_day_is_named_for_its_own_route() {
        let day = line();
        let rest = route_named("From stop · Day 2 Ulrichen", &(0..=10).map(|k| (0, k)).collect::<Vec<_>>());
        let src = SliceSource(&rest);
        let index = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&index, &src);
        let day_src = SliceSource(&day);
        let day_index = RouteIndex::read(&day_src).unwrap();
        let day_route = RouteReader::new(&day_index, &day_src);
        let trip = TripInput { id: 1, key: 42, name: "Alps", start_date: 0, stage_ids: &[7, 8] };
        let routes = [(day_route.summary(), 7), (day_route.summary(), 8), (route.summary(), 9)];
        let mut app = recording_on(&routes, &[trip], 2, |app| {
            let lead = super::super::LeadIn { splice: 9, route: 8, lead_m: 500, join_m: 0, rest_from_m: Some(0) };
            app.navigator.lead_in = Some(lead);
        });

        ride(&mut app, &route, &[(0, 0), (0, 5_000), (0, 9_900)]);
        assert_eq!(view(&app), Some(ArrivalView { route: 1, day: Some(1), next: None }), "named for Day 2's route");
        assert_eq!(obc_route::original_name("From stop · Day 2 Ulrichen"), "Day 2 Ulrichen");
        assert_eq!(obc_route::original_name("Detour · Grimsel"), "Grimsel");
        assert_eq!(obc_route::original_name("To start · Grimsel"), "Grimsel");
    }
}
