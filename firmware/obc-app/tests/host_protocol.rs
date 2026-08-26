//! What a **host** observes of the app's remaining protocol surface: the three-class residual a
//! typed executor still drains, the update door's single typed request, and the two pure
//! navigation-publication decisions the board's store task makes.
//!
//! The residual's own table (which three classes, and why each is still here) is
//! `device_core::residual`'s; the pass's guarantee that a drain between two passes cannot destroy an
//! admitted intent is `device_core::pass`'s. This file pins the host-side view of both.

use obc_app::dfu::DfuEffect;
use obc_app::{App, AppState, DrainStatus, HostCommand, HostMailbox};

mod common;
use common::quiet_pass;

/// A cancel queued while the store's synchronous publish is running must not turn the eventual
/// `Published` reply into a visible route. The host compensates that exact id before it considers
/// cancellation complete; without the late cancel the same reply activates normally.
#[test]
fn cancel_before_publish_result_requires_compensation() {
    use obc_app::host::{nav_publish_disposition, NavPublishDisposition};

    assert_eq!(nav_publish_disposition(false, 41), NavPublishDisposition::Activate(41));
    assert_eq!(nav_publish_disposition(true, 41), NavPublishDisposition::Compensate(41));
}

/// Compensation is idempotent with respect to an exact revision: success and `NotFound` both mean
/// the cancelled publication is gone, while retryable and terminal failures have distinct liveness
/// behavior. `Absent` covers the race where a later replacement already removed revision 1.
#[test]
fn publish_compensation_results_have_explicit_liveness() {
    use obc_app::host::{
        nav_compensation_disposition, NavCompensationDisposition as Disposition, NavCompensationStatus as Status,
    };

    assert_eq!(nav_compensation_disposition(Status::Removed), Disposition::Cancelled);
    assert_eq!(nav_compensation_disposition(Status::Absent), Disposition::Cancelled);
    assert_eq!(nav_compensation_disposition(Status::Retry), Disposition::Retry);
    assert_eq!(nav_compensation_disposition(Status::Terminal), Disposition::CancelledAfterTerminalFailure);
}

/// The residual drain asks for its three classes **by name**, and that is what makes it safe to run
/// between two passes: a class DeviceCore owns is not filtered out of a whole-order walk, it is
/// never reached at all. Here the rider has a planner request admitted and a bond removal pending —
/// the drain yields the bond removal, leaves Navigator's request where it was, and the next pass
/// hands the search out as the effect that carries it.
///
/// This is PR #1505's regression seen from the host: the whole-order walk *pulled* from Navigator on
/// the way past, minted the operation, and left the domain holding work nobody would ever answer.
#[test]
fn the_residual_drain_yields_the_bond_removal_and_never_reaches_a_pass_owned_class() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.debug_start_nav((0, 0), (1000, 1000), "Fountain North");
    app.state.ble_forget_pending = true; // the Bluetooth screen's guarded hold

    let mut mailbox: HostMailbox = HostMailbox::new();
    assert_eq!(app.drain_residual_commands(&mut mailbox), DrainStatus::Complete);
    assert_eq!(mailbox.pop(), Some(HostCommand::ForgetBond));
    assert!(mailbox.pop().is_none(), "the planner request is Navigator's — the drain never reaches it");

    let mut mailbox: HostMailbox = HostMailbox::new();
    let _ = app.drain_residual_commands(&mut mailbox);
    assert!(mailbox.is_empty(), "the bond removal is a one-shot the drain clears");

    // And the request the drain ran past is still there, for the pass to hand out.
    let mut plan = quiet_pass(&mut app, 10);
    assert!(plan.effects.navigator.take().is_some(), "the admitted plan reached the executor as an effect");
}

/// The remote-DFU door asks the update domain for exactly one scan; the open flow blocks a second
/// remote request, so the pass cannot hand out two.
#[test]
fn remote_dfu_check_reaches_the_executor_as_one_scan() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    assert!(app.open_remote_dfu_check());
    assert!(!app.open_remote_dfu_check(), "the open flow defers a second request");

    let mut plan = quiet_pass(&mut app, 10);
    assert!(matches!(plan.effects.dfu.take(), Some(DfuEffect::Scan { .. })), "one typed scan");

    let mut plan = quiet_pass(&mut app, 20);
    assert!(plan.effects.dfu.take().is_none(), "exactly once");
}
