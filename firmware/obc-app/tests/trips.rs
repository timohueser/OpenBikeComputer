//! Trip grouping (epic #526, TR2): resolving a host trip's stage route ids against the resident
//! route catalog into filed folders — the filed/unfiled partition, dangling refs, order, overflow,
//! and re-resolution across a route rescan.

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
    app.set_trips(&[TripInput { id: 1, name: "Alpen Traverse", stage_ids: &[7, 8] }]);

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
    // The flat catalog still holds all three (the flat menu is untouched until TR3).
    assert_eq!(app.routes().len(), 3);
}

/// A dangling stage ref (an id no route holds) drops from the resolved list and contributes nothing
/// to the stats — but the id stays in the verbatim `stage_ids`.
#[test]
fn dangling_ref_dropped_from_resolution() {
    let mut app = app_with_three_routes();
    app.set_trips(&[TripInput { id: 1, name: "Partial", stage_ids: &[7, 99, 8] }]);

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
    app.set_trips(&[TripInput { id: 5, name: "Ghost", stage_ids: &[98, 99] }]);

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
    app.set_trips(&[TripInput { id: 1, name: "Reversed", stage_ids: &[9, 7] }]);

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
    let inputs: Vec<TripInput> =
        (0..MAX_TRIPS + 5).map(|i| TripInput { id: i as u64, name: &names[i], stage_ids: &[7] }).collect();
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
    app.set_trips(&[TripInput { id: 1, name: "Growing", stage_ids: &[7, 8] }]);
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
