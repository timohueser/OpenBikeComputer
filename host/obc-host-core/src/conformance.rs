//! The **shared repository conformance suite** (#801): one set of assertions every host store —
//! the in-memory [`MemRouteStore`](crate::MemRouteStore) family and `obc-sim`'s folder-backed
//! stores — must pass, so both shapes prove the same identity-remap / delete / active-replacement /
//! nav-commit / track-lifecycle behaviour the shared dispatcher relies on. `obc-host-core`'s own
//! tests run it against the `Mem*` stores; `obc-sim`'s tests run it against the folder stores.
//!
//! Each entry takes a repository already seeded by the caller (the two store families seed from the
//! same committed route/ride object fixtures) plus the small extra facts a shape needs, and asserts the
//! store-family-independent invariants.

use obc_app::{App, AppState};

use crate::{RideRepository, RouteRepository, TrackRepository};

/// Route-store invariants: active-route replacement (the reparse signal), delete + id retirement,
/// and the reserved nav-route commit. `repo` must be seeded with **≥ 2** routes; `nav_bytes` is a
/// valid OBCR to commit as the computed route.
pub fn route_repository_suite(repo: &mut dyn RouteRepository, nav_bytes: &[u8]) {
    assert!(repo.catalog().len() >= 2, "seed the route conformance repo with ≥2 routes");
    assert_eq!(repo.catalog().len(), repo.ids().len(), "catalog and ids stay parallel");

    // Active-route replacement + the sync_active reparse signal.
    assert!(repo.sync_active(Some(0)), "first bind reports changed");
    assert!(repo.active_source().is_some(), "an active route serves bytes");
    assert!(!repo.sync_active(Some(0)), "an unchanged bind reports no change (no reparse)");
    assert!(repo.sync_active(Some(1)), "switching the active route reports changed");
    assert!(repo.sync_active(None), "clearing the active route reports changed");
    assert!(repo.active_source().is_none(), "no active route → no bytes");

    // A re-route rewrites bytes under an unchanged index: invalidate forces the next sync to re-read.
    assert!(repo.sync_active(Some(0)), "re-bind reports changed");
    repo.invalidate_active();
    assert!(repo.sync_active(Some(0)), "invalidate_active forces a re-read even under the same index");

    // Delete + id retirement (never reused).
    let victim = repo.ids()[0];
    let before = repo.catalog().len();
    assert!(repo.delete_by_id(victim), "a present id deletes");
    assert_eq!(repo.catalog().len(), before - 1, "the catalog shrinks by one");
    assert!(!repo.ids().contains(&victim), "the deleted id is gone from the catalog");
    assert!(!repo.delete_by_id(victim), "a retired id is a no-op");

    // The reserved nav-route commit appears in the catalog under a stable id.
    let nav_id = repo.write_nav_route(nav_bytes).expect("nav commit succeeds");
    assert!(repo.ids().contains(&nav_id), "the committed nav route carries an id in the catalog");
    let nav_id2 = repo.write_nav_route(nav_bytes).expect("a re-plan commits");
    assert_eq!(nav_id, nav_id2, "re-planning overwrites the reserved nav slot in place (stable id)");
}

/// The app-driven identity remap: with the app fed the seeded catalog and an active route selected,
/// deleting a *different* route and re-feeding keeps the active route on its durable id (the remap
/// the Route/Rides screens rely on). `repo` must be seeded with **≥ 2** routes.
pub fn route_identity_remap(repo: &mut dyn RouteRepository) {
    assert!(repo.catalog().len() >= 2, "seed the identity-remap repo with ≥2 routes");
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.set_routes_with_ids(repo.catalog(), repo.ids());
    app.activate_route(1);
    let active_id = repo.ids()[app.active_route_index().expect("route 1 active")];

    // Delete a route *before* the active one, so a positional index would drift but the id-keyed
    // remap must not.
    let victim = repo.ids()[0];
    assert_ne!(victim, active_id, "delete a different route than the active one");
    assert!(repo.delete_by_id(victim));
    app.set_routes_with_ids(repo.catalog(), repo.ids());

    let idx = app.active_route_index().expect("the active route survived the delete");
    assert_eq!(repo.ids()[idx], active_id, "the active route stays on its durable id across the remap");
}

/// Ride-store invariants: delete + id retirement, and the unknown-id track reads. `expects_track`
/// says whether a *known* id yields a real profile/preview (a folder-backed store with v3 ride
/// object bytes) — a memory store answers `None`/empty for every id.
pub fn ride_repository_suite(repo: &mut dyn RideRepository, expects_track: bool) {
    assert!(!repo.catalog().is_empty(), "seed the ride conformance repo with ≥1 ride");
    assert_eq!(repo.catalog().len(), repo.ids().len(), "catalog and ids stay parallel");

    // Unknown ids never read.
    let unknown = repo.ids().iter().copied().max().unwrap_or(0).wrapping_add(7);
    assert!(repo.profile_by_id(unknown).is_none(), "an unknown ride has no profile");
    assert!(repo.preview_by_id(unknown).is_empty(), "an unknown ride has no preview");

    let known = repo.ids()[0];
    if expects_track {
        assert!(repo.profile_by_id(known).is_some(), "a folder-backed ride yields its recorded profile");
        assert!(!repo.preview_by_id(known).is_empty(), "a folder-backed ride yields its recorded preview");
    } else {
        assert!(repo.profile_by_id(known).is_none(), "a memory ride has no on-disk profile");
        assert!(repo.preview_by_id(known).is_empty(), "a memory ride has no on-disk preview");
    }

    // Delete + id retirement.
    let before = repo.catalog().len();
    assert!(repo.delete_by_id(known), "a present ride deletes");
    assert_eq!(repo.catalog().len(), before - 1);
    assert!(!repo.delete_by_id(known), "a retired ride id is a no-op");
}

/// Track-lifecycle invariants, one per [`RecorderEffect`](obc_app::recorder::RecorderEffect) the
/// store serves: a session opens a log, a finalize closes it and names what it closed, a discard
/// closes it and names nothing. `has_sink` says whether the store exposes a recording sink (a folder
/// log does; a memory store never does).
pub fn track_lifecycle(tracks: &mut dyn TrackRepository, has_sink: bool) {
    // Nothing opened → nothing recording.
    assert!(tracks.sink().is_none(), "no ride → no sink");

    // A session opens a log.
    tracks.open(1, Some("ride"));
    assert_eq!(tracks.sink().is_some(), has_sink, "a recording ride exposes its sink iff the store has one");

    // The finalize closes it and answers with the identity it committed.
    let saved = tracks.finalize(stats());
    assert!(saved.is_some(), "a finalize that succeeded names the ride it committed");
    assert!(tracks.sink().is_none(), "and closes the log");

    // The next ride is a fresh object, and a discard leaves nothing behind.
    tracks.open(2, Some("ride"));
    assert_eq!(tracks.sink().is_some(), has_sink);
    tracks.discard();
    assert!(tracks.sink().is_none(), "a discard leaves nothing open");
}

/// Ride totals for the finalize above — the figures a v3 footer carries.
fn stats() -> obc_route::RideStats {
    obc_route::RideStats {
        distance_m: 1_000,
        moving_time_s: 300,
        avg_speed_cms: 333,
        climb_m: 20,
        unix_at_anchor: 1_720_000_000,
        anchor_ms: 0,
        clock_trusted: true,
        avg_hr: None,
        max_hr: None,
        avg_cadence: None,
        avg_power: None,
        max_power: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemRideStore, MemRouteStore, MemTrackStore};
    use obc_app::RideSummary;

    // Two distinct valid OBCR blobs the mem route store seeds from (the committed climb route, twice —
    // the bytes only need to parse as a `RouteSummary`; identity/index mechanics are what's under test).
    const ROUTE: &[u8] = include_bytes!("../../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr");

    #[test]
    fn mem_route_store_passes_the_conformance_suite() {
        let mut repo = MemRouteStore::new(&[ROUTE, ROUTE]);
        route_repository_suite(&mut repo, ROUTE);

        let mut repo = MemRouteStore::new(&[ROUTE, ROUTE]);
        route_identity_remap(&mut repo);
    }

    #[test]
    fn mem_ride_store_passes_the_conformance_suite() {
        let rides = vec![
            RideSummary {
                name: Default::default(),
                start_time: 2,
                distance_m: 10,
                moving_time_s: 5,
                climb_m: 1,
                synced: false,
                synced_at_utc: 0,
            },
            RideSummary {
                name: Default::default(),
                start_time: 1,
                distance_m: 20,
                moving_time_s: 6,
                climb_m: 2,
                synced: true,
                synced_at_utc: 0,
            },
        ];
        let mut repo = MemRideStore::new(rides);
        ride_repository_suite(&mut repo, false);
    }

    #[test]
    fn mem_track_store_passes_the_conformance_suite() {
        let mut tracks = MemTrackStore::new();
        track_lifecycle(&mut tracks, false);
    }
}
