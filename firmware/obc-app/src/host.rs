//! The **residual** app → host protocol (#1397 S6).
//!
//! DeviceCore's pass speaks bounded effects and token-carrying outcomes. What is left here is the
//! short list of things a typed executor still performs on the old protocol, because their domains
//! cannot validate an operation token and so cannot own an outcome:
//!
//! - [`HostCommand`] — three commands, drained through
//!   [`App::drain_residual_commands`](crate::App::drain_residual_commands) into a caller-owned
//!   [`HostMailbox`]. [`device_core::residual`](crate::device_core::residual) is the list as data,
//!   with the issue that retires each one.
//!
//! Each command has **exactly one** pending instance inside `App` — a typed slot or a flag, no
//! internal queue and no allocation — and all three are one-shots the drain clears. Draining is
//! loss-free: a command moves into the mailbox only if room exists, so a full mailbox leaves the
//! rest latched ([`DrainStatus::MailboxFull`]) rather than dropping one.
//!
//! Answers do not come back here. A ride close is answered by a catalog re-feed, a bond removal by
//! a link-status fact and a trip delete by the store's next revision — every one of them a fact the
//! pass already consumes.

use crate::activity::TrackAction;
use crate::device_core::residual::RESIDUAL_CLASS_COUNT;

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

/// The three things a typed executor still performs on the old protocol.
///
/// Payloads are bounded by construction: a durable `u16` object id and small `Copy` enums. No
/// catalog, profile, or geometry ever rides in a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostCommand {
    /// Cascade-delete the trip with durable object id `id` **and every member route** (epic #526,
    /// TR3). Still here because `CatalogState::admit_intent` refuses the cascade: the bounded
    /// member read does not exist yet (#1491). A vanished trip is a host-side no-op. One-shot,
    /// modal-flow-guarded.
    DeleteTrip { id: crate::CatalogObjectId },
    /// Close the open ride log: finalise it to the host's saved-ride artifact
    /// ([`TrackAction::Save`]) or throw it away ([`TrackAction::Discard`]). Still here because the
    /// close is answered by a catalog re-feed rather than by a ride identity, so Recorder has no
    /// outcome to validate (#1398). Persistence-critical one-shot; the host reads
    /// [`ride_stats`](crate::App::ride_stats) in the same pass so the wall-clock anchor pairs with
    /// the log's last points.
    FinishTrack(TrackAction),
    /// Forget the paired phone (epic #447, P8): clear the bond store and drop the bonded
    /// connection. Still here because the removal is confirmed by a link-status fact rather than by
    /// a reply (#1400). One-shot, guarded-hold-posted.
    ForgetBond,
}

/// One command class per [`HostCommand`] variant — [`RESIDUAL_CLASSES`] names them in the order the
/// drain asks for them.
///
/// [`RESIDUAL_CLASSES`]: crate::device_core::residual::RESIDUAL_CLASSES
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostCommandClass {
    DeleteTrip,
    FinishTrack,
    ForgetBond,
}

impl HostCommand {
    /// This command's class. The residual table's own cross-check is its only reader: production
    /// asks for a class by name and never asks a command what it is.
    #[cfg(test)]
    pub(crate) fn class(&self) -> HostCommandClass {
        match self {
            HostCommand::DeleteTrip { .. } => HostCommandClass::DeleteTrip,
            HostCommand::FinishTrack(_) => HostCommandClass::FinishTrack,
            HostCommand::ForgetBond => HostCommandClass::ForgetBond,
        }
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

/// What [`App::drain_residual_commands`](crate::App::drain_residual_commands) reports about a drain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum DrainStatus {
    /// Every pending command was moved into the mailbox.
    Complete,
    /// The mailbox filled before every class was drained. **Nothing was lost**: the remaining
    /// classes stay latched in the app and come out of the next drain — the saturation policy is
    /// backpressure, never a silent drop. Unreachable when the mailbox is empty and sized
    /// `N >= RESIDUAL_CLASSES.len()`.
    MailboxFull,
}

/// A caller-owned, compile-time-bounded FIFO of drained [`HostCommand`]s. The host allocates it
/// (stack or its own static — `App` never grows by it), fills it once per pass via
/// [`App::drain_residual_commands`](crate::App::drain_residual_commands), and pops it.
///
/// Nothing is coalesced: all three classes are one-shots, and each drained instance is a distinct
/// request.
#[derive(Debug)]
pub struct HostMailbox<const N: usize = RESIDUAL_CLASS_COUNT> {
    q: heapless::Deque<HostCommand, N>,
}

impl<const N: usize> HostMailbox<N> {
    /// An empty mailbox.
    pub const fn new() -> Self {
        HostMailbox { q: heapless::Deque::new() }
    }

    /// Pop the next command in canonical order, or `None` when empty.
    pub fn pop(&mut self) -> Option<HostCommand> {
        self.q.pop_front()
    }

    /// How many commands are queued (clippy pairs it with [`is_empty`](Self::is_empty); the
    /// protocol tests assert exact batch sizes through it).
    pub fn len(&self) -> usize {
        self.q.len()
    }

    /// Whether the mailbox is empty.
    pub fn is_empty(&self) -> bool {
        self.q.is_empty()
    }

    /// Whether the mailbox is full — the drain's backpressure signal.
    pub fn is_full(&self) -> bool {
        self.q.is_full()
    }

    /// Queue one drained command. Returns `false` — leaving the command with the caller — only when
    /// the mailbox is full (the drain checks room first, so its pushes never fail).
    pub(crate) fn push(&mut self, cmd: HostCommand) -> bool {
        self.q.push_back(cmd).is_ok()
    }
}

impl<const N: usize> Default for HostMailbox<N> {
    fn default() -> Self {
        Self::new()
    }
}

// Layout tripwire: a residual command is an id or a small enum, never a catalog or profile. The
// mailbox is caller-owned, so `App` grows by none of this.
const _: () = assert!(core::mem::size_of::<HostCommand>() <= 16, "HostCommand grew — re-check the payload budget");
