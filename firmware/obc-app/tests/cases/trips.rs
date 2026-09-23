//! Trip grouping: resolving a host trip's stage route ids against the resident route catalog into
//! filed folders — the filed/unfiled partition, dangling refs, order, overflow, and re-resolution
//! across a route rescan.

use obc_app::trip::{RouteVersion, TripProgress};
use obc_app::{App, AppState, RouteSummary, TripInput, MAX_TRIPS};
use obc_map_scene::BBox;

fn route(name: &str, distance_km: u32, climb_m: u32) -> RouteSummary {
    let mut n = heapless::String::<48>::new();
    let _ = n.push_str(name);
    RouteSummary {
        name: n,
        distance_km,
        climb_m,
        bbox: BBox { min_lon: 0, min_lat: 0, max_lon: 1000, max_lat: 1000 },
        start_lon: 100,
        start_lat: 100,
    }
}

/// Three routes with durable ids 7, 8, 9 (distances 10/20/30 km, climbs 100/200/300 m).
fn three_routes() -> ([RouteSummary; 3], [u64; 3]) {
    ([route("Alpha", 10, 100), route("Beta", 20, 200), route("Gamma", 30, 300)], [7, 8, 9])
}

fn app_with_three_routes() -> App {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    let (routes, ids) = three_routes();
    app.set_routes_with_ids(&routes, &ids);
    app
}

/// A trip grouping routes 7 & 8 files exactly those two; route 9 stays unfiled. The resolved trip
/// carries the catalog indices (ride order) and the summed distance/climb over its stages.
#[test]
fn filed_vs_unfiled_partition() {
    let mut app = app_with_three_routes();
    app.set_trips(&[TripInput { id: 1, key: 1, name: "Alpen Traverse", start_date: 0, stage_ids: &[7, 8] }]);

    assert_eq!(app.trips().len(), 1);
    let t = &app.trips()[0];
    assert_eq!(t.id, 1);
    assert_eq!(t.name, "Alpen Traverse");
    assert_eq!(t.stage_indices.as_slice(), &[0, 1]); // routes 7,8 sit at catalog 0,1
    assert_eq!(t.distance_km, 30); // 10 + 20
    assert_eq!(t.climb_m, 300); // 100 + 200
    assert!(!t.is_empty_folder());

    // Routes 7 & 8 are filed; route 9 is not — the top level would list the trip + route 9.
    assert!(app.route_filed(0));
    assert!(app.route_filed(1));
    assert!(!app.route_filed(2));
    // The flat catalog still holds all three.
    assert_eq!(app.routes().len(), 3);
}

/// A dangling stage ref (an id no route holds) drops from the resolved list and contributes nothing
/// to the stats — but the id stays in the verbatim `stage_ids`.
#[test]
fn dangling_ref_dropped_from_resolution() {
    let mut app = app_with_three_routes();
    app.set_trips(&[TripInput { id: 1, key: 1, name: "Partial", start_date: 0, stage_ids: &[7, 99, 8] }]);

    let t = &app.trips()[0];
    assert_eq!(t.stage_ids.as_slice(), &[7, 99, 8]); // stored verbatim, dangling included
    assert_eq!(t.stage_indices.as_slice(), &[0, 1]); // only 7 & 8 resolve
    assert_eq!(t.distance_km, 30); // dangling 99 sums nothing
    assert_eq!(t.climb_m, 300);
    assert!(!t.is_empty_folder());
}

/// A trip whose every ref dangles still lists — an empty folder, so it can be deleted on-device —
/// with zeroed stats.
#[test]
fn fully_dangling_trip_still_lists() {
    let mut app = app_with_three_routes();
    app.set_trips(&[TripInput { id: 5, key: 1, name: "Ghost", start_date: 0, stage_ids: &[98, 99] }]);

    assert_eq!(app.trips().len(), 1);
    let t = &app.trips()[0];
    assert!(t.is_empty_folder());
    assert!(t.stage_indices.is_empty());
    assert_eq!(t.stage_ids.as_slice(), &[98, 99]); // kept, so the folder is still deletable
    assert_eq!(t.distance_km, 0);
    assert_eq!(t.climb_m, 0);
    // Nothing is filed.
    assert!(!app.route_filed(0));
    assert!(!app.route_filed(1));
    assert!(!app.route_filed(2));
}

/// Stage order is the trip's ride order, not the catalog order: a trip listing 9 before 7 resolves
/// to catalog indices [2, 0].
#[test]
fn stage_order_preserved() {
    let mut app = app_with_three_routes();
    app.set_trips(&[TripInput { id: 1, key: 1, name: "Reversed", start_date: 0, stage_ids: &[9, 7] }]);

    let t = &app.trips()[0];
    assert_eq!(t.stage_indices.as_slice(), &[2, 0]);
    assert_eq!(t.distance_km, 40); // 30 + 10
}

/// More trips than the resident cap: `set_trips` keeps the first `MAX_TRIPS` (warn + first N is the
/// host's job on the scan, mirroring the route-scan overflow).
#[test]
fn max_trips_overflow_keeps_first_n() {
    let mut app = app_with_three_routes();
    let names: Vec<String> = (0..MAX_TRIPS + 5).map(|i| format!("Trip {i}")).collect();
    let inputs: Vec<TripInput> = (0..MAX_TRIPS + 5)
        .map(|i| TripInput { id: i as u64, key: 1, name: &names[i], start_date: 0, stage_ids: &[7] })
        .collect();
    app.set_trips(&inputs);

    assert_eq!(app.trips().len(), MAX_TRIPS);
    // The first N survived, in order.
    assert_eq!(app.trips()[0].id, 0);
    assert_eq!(app.trips()[MAX_TRIPS - 1].id, (MAX_TRIPS - 1) as u64);
}

/// A trip resolves lazily against the *current* catalog: a stage id that dangles when the trip is
/// set becomes filed once its route appears on a rescan (and re-dangles if the route vanishes),
/// without the host re-feeding the trips.
#[test]
fn reresolves_across_a_route_rescan() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    // Route 8 is not present yet.
    app.set_routes_with_ids(&[route("Alpha", 10, 100)], &[7]);
    app.set_trips(&[TripInput { id: 1, key: 1, name: "Growing", start_date: 0, stage_ids: &[7, 8] }]);
    {
        let t = &app.trips()[0];
        assert_eq!(t.stage_indices.as_slice(), &[0]); // only 7 resolves
        assert_eq!(t.distance_km, 10);
    }

    // Route 8 lands on a rescan — the trip re-resolves in place.
    let (routes, ids) = three_routes();
    app.set_routes_with_ids(&routes, &ids);
    {
        let t = &app.trips()[0];
        assert_eq!(t.stage_indices.as_slice(), &[0, 1]);
        assert_eq!(t.distance_km, 30);
        assert!(app.route_filed(0));
        assert!(app.route_filed(1));
    }

    // Route 8 is deleted (catalog now 7, 9) — stage 8 re-dangles.
    app.set_routes_with_ids(&[route("Alpha", 10, 100), route("Gamma", 30, 300)], &[7, 9]);
    {
        let t = &app.trips()[0];
        assert_eq!(t.stage_indices.as_slice(), &[0]); // 7 at catalog 0; 8 gone
        assert_eq!(t.distance_km, 10);
        assert!(!app.route_filed(1)); // catalog index 1 is now route 9 — unfiled
    }
}

/// A ride records the bike type that is current at its start, and the trip day when the loaded
/// route is a day route. A ride on a loose route records no trip.
#[test]
fn a_ride_records_its_trip_day_and_bike_type() {
    use obc_app::{RecorderIntent, Settings};
    use obc_formats::{bike::BikeType, ride::TripRef};

    let start_on = |route: usize| {
        let mut app = app_with_three_routes();
        app.set_trips(&[TripInput { id: 1, key: 42, name: "Alpen Traverse", start_date: 0, stage_ids: &[7, 8] }]);
        app.set_settings(Settings { bike_type: BikeType::Gravel, ..Settings::default() });
        crate::common::mount_store(&mut app);
        app.activate_route(route);
        app.recorder.request(RecorderIntent::Start);
        crate::common::quiet_pass(&mut app, 1);
        assert!(app.recording());
        let stats = app.ride_stats();
        (stats.bike, stats.trip, stats.trip_name)
    };

    let (bike, trip, name) = start_on(1);
    assert_eq!((bike, trip, name.as_str()), (BikeType::Gravel, TripRef::new(42, 1, 2), "Alpen Traverse"));
    let (bike, trip, name) = start_on(2);
    assert_eq!((bike, trip, name.as_str()), (BikeType::Gravel, None, ""));
}

/// Ride the route at catalog index `route` over a store that takes every write, and save the ride
/// when `save`. `ride_on` loads the next route during the ride, as Ride on in the arrival view does.
/// Returns the progress records the store takes, in write order.
fn ride(app: &mut App, route: usize, ride_on: bool, save: bool) -> Vec<TripProgress> {
    use obc_app::catalog_state::{CatalogEffect, CatalogOutcome};
    use obc_app::device_core::{ExternalFacts, OutcomeSlots, Revision, StoreIdentity, StoreRevision};
    use obc_app::metadata::{MetadataEffect, MetadataOutcome};
    use obc_app::recorder::{CheckpointStatus, RecorderEffect, RecorderOutcome};
    use obc_app::RecorderIntent;

    app.activate_route(route);
    let scope = StoreRevision { store: StoreIdentity::new(1), revision: Revision::new(1) };
    let mut outcomes = OutcomeSlots::new();
    let mut written = Vec::new();
    for pass in 0..40 {
        let mut facts = ExternalFacts::NONE;
        facts.note_store_revision(scope);
        match pass {
            1 => app.recorder.request(RecorderIntent::Start),
            2 if ride_on => app.activate_route(route + 1),
            3 if save => app.recorder.request(RecorderIntent::Save),
            _ => {}
        }
        let mut plan = crate::common::pass(app, pass * 1_000, &mut outcomes, &mut facts, None);
        if let Some(CatalogEffect::ReadCatalog { token }) = plan.effects.catalog.take() {
            outcomes.catalog.try_put(CatalogOutcome::CatalogRead { token, scope: Some(scope) }).unwrap();
        }
        match plan.effects.recorder.take() {
            Some(RecorderEffect::Append { token, samples }) => {
                outcomes.recorder.try_put(RecorderOutcome::Appended { token, samples }).unwrap()
            }
            Some(RecorderEffect::Checkpoint { token }) => outcomes
                .recorder
                .try_put(RecorderOutcome::Checkpointed { token, status: CheckpointStatus::Durable })
                .unwrap(),
            Some(RecorderEffect::Finalize { token }) => {
                outcomes.recorder.try_put(RecorderOutcome::Finalized { token, ride: 77 }).unwrap()
            }
            _ => {}
        }
        if let Some(MetadataEffect::WriteProgress { token, .. }) = plan.effects.metadata.take() {
            written.extend(app.trip_progress_payload(token).cloned());
            outcomes.metadata.try_put(MetadataOutcome::ProgressWritten { token }).unwrap();
        }
    }
    written
}

/// Ride Day 2 of a three-day trip and save it. Returns the progress record the Finish writes.
fn finish_day_2(ride_on: bool) -> (App, TripProgress) {
    let mut app = app_with_three_routes();
    app.set_trips(&[TripInput { id: 1, key: 42, name: "Alpen Traverse", start_date: 0, stage_ids: &[7, 8, 9] }]);
    let written = ride(&mut app, 1, ride_on, true);
    let finish = written.last().cloned().expect("the Finish writes the trip's progress");
    (app, finish)
}

/// A ride that starts on a day of another trip makes that trip active before any Finish: the start
/// card's day row follows it, and the store takes the moved record, so a power cycle keeps it. A
/// ride on the active trip writes nothing at its start.
#[test]
fn a_ride_on_a_day_of_another_trip_makes_that_trip_active() {
    let trips = || {
        let mut app = app_with_three_routes();
        app.set_trips(&[
            TripInput { id: 1, key: 42, name: "Alps", start_date: 0, stage_ids: &[7, 8] },
            TripInput { id: 2, key: 5, name: "Jura", start_date: 0, stage_ids: &[9] },
        ]);
        app.set_trip_progress([TripProgress {
            key: 42,
            day: 1,
            day_route: RouteVersion { id: 8, revision: 1 },
            metres: 0,
            last_finished: Some(0),
            dates: [0; obc_route::MAX_TRIP_DAYS],
        }]);
        assert_eq!(app.next_trip_day().map(|(t, day)| (t.key, day)), Some((42, 1)));
        app
    };

    let mut app = trips();
    let written = ride(&mut app, 2, false, false);
    assert_eq!(written.iter().map(|p| (p.key, p.last_finished)).collect::<Vec<_>>(), [(5, None)]);
    assert_eq!(app.next_trip_day().map(|(t, day)| (t.key, day)), Some((5, 0)));
    assert_eq!(app.trip_progress().iter().map(|p| p.key).collect::<Vec<_>>(), [42, 5], "Alps keeps its record");

    let mut app = trips();
    assert!(ride(&mut app, 1, false, false).is_empty());
}

/// Finish of a ride on a trip day moves the trip's progress: the store gets one record, and the
/// resident records name the next day at once.
#[test]
fn a_saved_ride_on_a_trip_day_writes_the_trip_progress() {
    let (app, written) = finish_day_2(false);
    assert_eq!((written.key, written.day, written.day_route.id, written.last_finished), (42, 1, 8, Some(1)));
    let trip = &app.trips()[0];
    assert_eq!(trip.next_day(trip.progress_in(app.trip_progress())), Some(2), "Day 3 is next");
}

/// A ride that rode on into Day 3 finishes Day 2 and leaves the position in Day 3.
#[test]
fn a_ride_that_rode_on_leaves_the_position_in_the_next_day() {
    let (_, written) = finish_day_2(true);
    assert_eq!((written.day, written.day_route.id, written.last_finished), (2, 9, Some(1)));
}
