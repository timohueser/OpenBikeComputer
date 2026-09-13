//! Host planning values.

/// What the board host must do when a computed-route publication answers. Cancellation can arrive
/// while the synchronous store task is committing, so the answer is not automatically a success:
/// the just-published revision must be removed before the host reports the cancellation complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavPublishDisposition {
    Activate(crate::CatalogObjectId),
    Compensate(crate::CatalogObjectId),
}

pub const fn nav_publish_disposition(cancel_requested: bool, id: crate::CatalogObjectId) -> NavPublishDisposition {
    if cancel_requested {
        NavPublishDisposition::Compensate(id)
    } else {
        NavPublishDisposition::Activate(id)
    }
}

/// Store-task result categories relevant to retracting a route whose publication raced cancel.
/// Kept independent of a concrete store error type so the app/host state machine remains portable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavCompensationStatus {
    /// The exact published revision was removed.
    Removed,
    /// The exact revision is already absent (for example a later replacement removed it first).
    Absent,
    /// Media or scheduling failure that can succeed on a later pass.
    Retry,
    /// A permanent store refusal. The host must release its planner resources rather than spin
    /// forever; the board logs this as a violated publish-capacity invariant.
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavCompensationDisposition {
    Cancelled,
    Retry,
    CancelledAfterTerminalFailure,
}

pub const fn nav_compensation_disposition(status: NavCompensationStatus) -> NavCompensationDisposition {
    match status {
        NavCompensationStatus::Removed | NavCompensationStatus::Absent => NavCompensationDisposition::Cancelled,
        NavCompensationStatus::Retry => NavCompensationDisposition::Retry,
        NavCompensationStatus::Terminal => NavCompensationDisposition::CancelledAfterTerminalFailure,
    }
}

/// A planned detour's preview figures (#882), carried by
/// [`NavigatorOutcome::DetourFinished`](crate::navigator::NavigatorOutcome): the cost
/// delta the HUD line shows (`detour length − skipped span length`, signed — a detour around a
/// wandering span *can* be shorter), the detour's own length, and — since #1091 — its own climb,
/// which the preview turns into the second signed figure beside the distance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetourPreview {
    /// `detour_total − (rejoin_m − progress_m)`, meters.
    pub cost_delta_m: i32,
    /// The planned detour's honest length (summed raw edge meters).
    pub total_distance_m: u32,
    /// Where the plan actually rejoins the route — the chooser's `target_m`, or farther when the
    /// approach was trimmed to its first sustained tail contact. The replaced span the climb
    /// figure subtracts is `[anchor_m, rejoin_m]`, so it describes the same swap
    /// [`cost_delta_m`](Self::cost_delta_m) already prices.
    pub rejoin_m: u32,
    /// The planned detour's own dead-banded ascent (m), or `None` when **no terrain sample
    /// resolved** for it — the producer's explicit
    /// [`RouteStats::has_elevation`](obc_route::RouteStats), never a guess at the values, because a
    /// genuinely flat detour is `Some(0)` and must still show a figure.
    pub ascent_m: Option<u32>,
}
