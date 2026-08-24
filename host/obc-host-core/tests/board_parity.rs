//! Board ↔ dispatcher protocol-ordering parity (#801).
//!
//! The board keeps its **async** ride loop and its per-latch `take_*` drains (#801 non-goal; #809
//! owns the board loop), while the frame-stepped hosts run the shared [`HostLoop`] dispatcher. Both
//! consume the *same* typed `HostCommand` protocol, so the orderings the board hand-codes must hold
//! through the dispatcher too. The class-order / annihilation / counted-rescan mechanics are pinned
//! crate-internally in `obc-app` (`tests/host_protocol.rs`); this file pins that the **dispatcher**
//! reproduces the board's two load-bearing sequences:
//!
//! - `RescanStore` re-feeds the catalog before subsequent work (the board's
//!   `take_store_changed → refeed`), so an upload/delete id resolves against the rescanned catalog.
//! - `PlanRoute` is consumed into the resumable planner (the board's `PlanRoute → plan step`),
//!   and a `CancelRoutePlan` posted in the same input batch **annihilates** it, so the dispatcher
//!   never starts a plan the rider already dismissed.

#![cfg(feature = "external-fixtures")]

use obc_app::{App, AppState};
use obc_host_core::trace::{reconcile_fixture_pass, reconcile_fixture_to_completion};
use obc_host_core::{HostLoop, MemRouteStore};

/// The board rescans the object store on a store-changed edge and re-feeds the catalog; the
/// dispatcher's `RescanStore` must do the same, so a subsequent id resolves against the rescanned
/// (not the stale) catalog.
#[test]
fn rescan_refeeds_the_catalog_like_the_board() {
    let map = obc_fixtures::read("sim-grimsel", "grimsel.obcm").expect("full fixture suite requires map");
    let route =
        obc_fixtures::read("sim-grimsel", "routes/grimsel-climb.obcr").expect("full fixture suite requires route");
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    let mut routes = MemRouteStore::new(&[&route, &route]);
    app.set_routes_with_ids(routes.catalog(), routes.ids());
    assert_eq!(app.routes().len(), 2, "the app sees both seeded routes");

    // An external store mutation (a delete committed elsewhere) plus the store-changed signal — the
    // board's `notify_store_changed` edge.
    let gone = routes.ids()[0];
    assert!(routes.delete_by_id(gone));
    app.apply_event(obc_app::HostEvent::StoreChanged);

    let mut host = HostLoop::new();
    reconcile_fixture_pass(&mut host, &mut app, &mut routes, &map).expect("grimsel map parses");

    assert_eq!(app.routes().len(), 1, "the dispatcher's RescanStore re-fed the rescanned catalog");
    assert!(!app.route_ids().contains(&gone), "the deleted id is gone from the app catalog too");
}

/// The board consumes `PlanRoute` into its one-step-per-pass planner; the dispatcher consumes
/// `PlanRoute` into the resumable [`NavPlan`] the same way (`is_planning` after the pass).
#[test]
fn plan_route_enters_the_resumable_planner_like_the_board() {
    let map = obc_fixtures::read("sim-grimsel", "grimsel.obcm").expect("full fixture suite requires map");
    let route =
        obc_fixtures::read("sim-grimsel", "routes/grimsel-climb.obcr").expect("full fixture suite requires route");
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    let mut routes = MemRouteStore::new(&[&route]);
    app.set_routes_with_ids(routes.catalog(), routes.ids());
    app.debug_start_nav((8_330_000, 46_570_000), (8_340_000, 46_575_000), "Parity Plan");

    let mut host = HostLoop::new();
    assert!(!host.is_planning(), "nothing planning before the pass");
    reconcile_fixture_pass(&mut host, &mut app, &mut routes, &map).expect("grimsel map parses");
    assert!(host.is_planning(), "the dispatcher consumed PlanRoute into the resumable planner");
}

/// A headless completion pass emits the exact same reserved route bytes as repeated frame passes;
/// only the yield cadence differs.
#[test]
fn completion_matches_repeated_frame_steps() {
    let map = obc_fixtures::read("sim-grimsel", "grimsel.obcm").expect("full fixture suite requires map");
    let route =
        obc_fixtures::read("sim-grimsel", "routes/grimsel-climb.obcr").expect("full fixture suite requires route");
    let run = |complete: bool| {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let mut routes = MemRouteStore::new(&[&route]);
        app.set_routes_with_ids(routes.catalog(), routes.ids());
        app.debug_start_nav((8_169_610, 46_694_536), (8_217_309, 46_706_261), "Same Plan");
        let mut host = HostLoop::new();
        if complete {
            reconcile_fixture_to_completion(&mut host, &mut app, &mut routes, &map).expect("grimsel map parses");
        } else {
            for _ in 0..10_000 {
                reconcile_fixture_pass(&mut host, &mut app, &mut routes, &map).expect("grimsel map parses");
                if !host.is_planning() {
                    break;
                }
            }
        }
        assert!(!host.is_planning(), "planner reaches a terminal result");
        routes.sync_active(app.active_route_index());
        routes.active_source().expect("route committed").0.to_vec()
    };
    assert_eq!(run(false), run(true), "yield cadence must not change the committed OBCR");
}

/// A `debug_start_nav` immediately dismissed (the confirm→Back annihilation, #837) leaves the
/// dispatcher with **no** plan: the cancel clears the undrained request at post time, exactly as it
/// does for the board's `CancelRoutePlan` before `PlanRoute`.
#[test]
fn cancel_before_the_pass_starts_no_plan() {
    let map = obc_fixtures::read("sim-grimsel", "grimsel.obcm").expect("full fixture suite requires map");
    let route =
        obc_fixtures::read("sim-grimsel", "routes/grimsel-climb.obcr").expect("full fixture suite requires route");
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    let mut routes = MemRouteStore::new(&[&route]);
    app.set_routes_with_ids(routes.catalog(), routes.ids());
    app.debug_start_nav((8_330_000, 46_570_000), (8_340_000, 46_575_000), "Dismissed Plan");
    // The rider dismisses the planning screen in the same batch (Back on NavPlanning), annihilating
    // the undrained request through the real screen path.
    app.apply_gesture(obc_app::Gesture::Back);

    let mut host = HostLoop::new();
    reconcile_fixture_pass(&mut host, &mut app, &mut routes, &map).expect("grimsel map parses");
    assert!(!host.is_planning(), "an annihilated request never starts a plan in the dispatcher");
}
