//! The typed app↔host protocol (FAR-07, #800), exercised host-side through the public API:
//! [`App::drain_host_commands`] / [`HostMailbox`] on the command side and [`App::apply_event`] on
//! the answer side. The class-order / coalescing / saturation / single-pending-state mechanics are
//! pinned by the crate-internal unit tests; this file pins what a *host* observes — deferred owned
//! answers, cancel-before-plan ordering, drop-if-gone answers, and compat/typed equivalence.

use obc_app::screen::Screen;
use obc_app::{App, AppState, DrainStatus, Gesture, HostCommand, HostEvent, HostMailbox, RouteSummary};
use obc_reader::BBox;

/// A one-route catalog under durable id 7 — the rescan the host performs before answering a plan.
fn nav_catalog(app: &mut App) {
    let mut name = heapless::String::<48>::new();
    let _ = name.push_str("Fountain North");
    let sum = RouteSummary {
        name,
        distance_km: 1,
        climb_m: 0,
        bbox: BBox { min_lon: 0, min_lat: 0, max_lon: 1000, max_lat: 1000 },
        start_lon: 0,
        start_lat: 0,
    };
    app.set_routes_with_ids(&[sum], &[7]);
}

/// Drain into a fresh canonical-capacity mailbox, asserting completeness.
fn drain(app: &mut App) -> HostMailbox {
    let mut mailbox: HostMailbox = HostMailbox::new();
    assert_eq!(app.drain_host_commands(&mut mailbox), DrainStatus::Complete);
    mailbox
}

/// A drained [`HostCommand::PlanRoute`] is an **owned** value: the host can park it, keep driving
/// the app, and answer many passes later with an owned [`HostEvent`] — no borrow into `App` at any
/// point, and the late answer lands exactly like the legacy `notify_nav_result`.
#[test]
fn a_host_defers_the_answer_without_borrowing_the_app() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.debug_start_nav((0, 0), (1000, 1000), "Fountain North");

    let mut mailbox = drain(&mut app);
    let Some(HostCommand::PlanRoute(req)) = mailbox.pop() else { panic!("the plan request drains typed") };
    assert!(mailbox.pop().is_none(), "…and nothing else is pending");
    assert_eq!(req.name(), "Fountain North");

    // The "async plan" runs across further app activity (input keeps flowing).
    app.apply_gesture(Gesture::Turn(1));
    assert!(drain(&mut app).is_empty(), "no re-emission while the host holds the request");

    // Answer whole passes later: rescan first, then the owned event — the same ordering contract.
    nav_catalog(&mut app);
    app.apply_event(HostEvent::NavPlanned(Ok(7)));
    assert!(matches!(app.top_screen(), Screen::RouteOverview(_)), "the late answer lands in the planning screen");
    assert_eq!(app.activity.active_route, Some(0), "…and activates the committed route");
}

/// A confirm and its Back applied in one batch **before any drain** (reachable during a long
/// render pass) net "no plan": the cancel annihilates the undrained request through the real
/// screen path, so the host never executes a dismissed plan and no ghost route can commit.
#[test]
fn same_batch_confirm_and_back_net_no_plan() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.debug_start_nav((0, 0), (1000, 1000), "Dismissed before drained");
    app.apply_gesture(Gesture::Back); // Back on the spinner, same batch — no drain in between

    let mut mailbox = drain(&mut app);
    assert_eq!(mailbox.pop(), Some(HostCommand::CancelRoutePlan), "only the (no-op) cancel drains");
    assert!(mailbox.pop().is_none(), "the dismissed plan never reaches the host");
    assert!(app.take_nav_request().is_none() && !app.take_nav_cancel(), "…and nothing is left behind");
}

/// Back mid-plan pops the spinner and posts the cancel; a plan posted **after** the cancel
/// survives it (annihilation only kills a request the cancel's Back was aimed at), the typed
/// drain yields `CancelRoutePlan` before that `PlanRoute` (the canonical order), and a defensive
/// post-cancel answer through the typed door is dropped exactly like the legacy seam.
#[test]
fn cancel_drains_before_a_new_plan_and_a_late_answer_is_dropped() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.debug_start_nav((0, 0), (1000, 1000), "First try");
    let _ = drain(&mut app); // the host holds the first plan

    app.apply_gesture(Gesture::Back); // rider cancels the spinner
    app.debug_start_nav((0, 0), (2000, 2000), "Second try"); // …and immediately asks again

    let mut mailbox = drain(&mut app);
    assert_eq!(mailbox.pop(), Some(HostCommand::CancelRoutePlan), "cancellation precedes new work");
    assert!(matches!(mailbox.pop(), Some(HostCommand::PlanRoute(_))), "the fresh plan follows");

    // The host aborts plan one but had already finished a step: its late answer finds the *new*
    // planning screen — the answer targets whatever plan is live, exactly as before. Cancel that
    // one too and the answer is dropped outright.
    app.apply_gesture(Gesture::Back);
    assert_eq!(drain(&mut app).pop(), Some(HostCommand::CancelRoutePlan));
    nav_catalog(&mut app);
    app.apply_event(HostEvent::NavPlanned(Ok(7)));
    assert!(!matches!(app.top_screen(), Screen::RouteOverview(_)), "a post-cancel answer is dropped");
    assert_eq!(app.activity.active_route, None, "nothing activates after a cancel");
}

/// The remote-DFU door posts a typed `Dfu(Scan)` exactly once; the open flow blocks a second
/// remote request, so the command cannot double-emit.
#[test]
fn remote_dfu_check_drains_as_one_typed_scan() {
    use obc_app::DfuAction;
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    assert!(app.open_remote_dfu_check());
    assert!(!app.open_remote_dfu_check(), "the open flow defers a second request");

    let mut mailbox = drain(&mut app);
    assert_eq!(mailbox.pop(), Some(HostCommand::Dfu(DfuAction::Scan)));
    assert!(mailbox.pop().is_none());
    assert!(drain(&mut app).is_empty(), "exactly once");
}

/// One pending state, two doors: a store-changed burst drained typed leaves the compat counter
/// empty, and a Forget-phone drained compat leaves the typed protocol empty.
#[test]
fn typed_and_compat_doors_share_one_pending_state() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));

    app.apply_event(HostEvent::StoreChanged);
    app.notify_store_changed(); // the compat feeder increments the same counter
    let mut mailbox = drain(&mut app);
    assert_eq!(mailbox.pop(), Some(HostCommand::RescanStore { commits: 2 }), "both feeds, one counted command");
    assert_eq!(app.take_store_changed(), 0, "the typed drain emptied the compat counter");

    app.state.ble_forget_pending = true; // the Bluetooth screen's guarded hold
    assert!(app.take_ble_forget());
    assert!(drain(&mut app).is_empty(), "the compat take emptied the typed protocol");
    assert!(!app.has_pending_host_command());
}
