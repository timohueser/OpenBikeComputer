//! Host navigation publication and remote update contracts.

use obc_app::dfu::DfuEffect;
use obc_app::{App, AppState};

use crate::common::quiet_pass;

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
