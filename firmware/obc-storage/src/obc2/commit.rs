//! §4's `CommitEvent`: the in-process catch-up signal a repository emits after a catalog commit.
//!
//! `Device_Object_System_v2.md` §4 is the whole specification of this file, and it is short:
//!
//! > `CommitEvent` is that catch-up signal and nothing more: an in-process notification a repository
//! > emits after a catalog commit's validity gate is durable, carrying StoreId, repository,
//! > ObjectKind, LogicalObjectId, the new Revision, and the operation's terminal outcome. It is not
//! > a wire message, has no frame, opcode, or codec, and no peer subscribes to it; a client learns
//! > the same facts by querying the catalog or the operation. Consecutive events for one repository
//! > may be coalesced into the latest Revision, because a consumer that reads the current state
//! > loses nothing by skipping intermediate ones. A consumer that missed events across a reboot
//! > obtains the identical catch-up from the recovered repository Revision, so no durable event
//! > queue exists.
//!
//! Three things follow from that paragraph, and [`CommitLog`] is built out of them rather than out
//! of a general-purpose queue:
//!
//! 1. **The queue is indexed by repository, not by event.** Coalescing "into the latest Revision" is
//!    not a policy applied when a buffer fills — it is the shape of the structure. One slot per
//!    repository holds the newest event, a later event for the same repository replaces it, and
//!    there is therefore no capacity to exhaust and no such thing as a dropped edge: the state a
//!    consumer would have read after the event it missed is exactly the state it reads after the one
//!    that replaced it.
//! 2. **Delivery is a retained revision plus a wake, not a stream.** [`CommitLog::latest`] answers
//!    "what does this repository stand at" whenever it is asked, and stays answerable after the wake
//!    has been taken; [`CommitLog::take`] is only the edge that says a consumer has something to do.
//! 3. **Nothing here is durable.** A reboot loses the log, and §4 says that costs nothing: the
//!    recovered repository Revision is the identical catch-up.
//!
//! It is not a wire message and there is no codec here for the same reason there is no `encode`: a
//! peer that wants these facts asks `QueryCatalog` or `QueryOperation`.

use heapless::Vec;

use obc_link::ids::{LogicalObjectId, OperationId, Revision, StoreId};
use obc_link::registry::{ObjectKind, ObjectOutcome};

/// The number of repositories one store has: exactly one per registered [`ObjectKind`].
pub const REPOSITORIES: usize = ObjectKind::ALL.len();

/// What a durable commit did to the head it names.
///
/// §4 carries "the operation's terminal outcome" and #1359 carries a change kind; they are the same
/// fact at two resolutions, and this is the finer one — `Created` and `Replaced` are the two halves
/// of the wire's single `Committed`, and the store knows which because the claim named a create or a
/// compare-and-swap replace.
///
/// #1359's list also names `ManifestActivated` and `UpdateReady`. Neither is here, deliberately: a
/// manifest publication *is* a create-or-replace of the volume-manifest repository and a verified
/// update package *is* a create of the update repository, and #1359 admits a distinct kind "only if
/// a consumer proves the distinction". DOS7 and DOS8 own those consumers; inventing the variants
/// before them would freeze a distinction nothing can yet justify.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// A head that did not exist now does.
    Created,
    /// A head was replaced by a new immutable generation.
    Replaced,
    /// A head was removed.
    Deleted,
    /// Only the catalog projection changed.
    MetadataChanged,
    /// An update install was requested; the head is unchanged and the repository revision did not
    /// move, which is exactly why a consumer still wants the edge.
    InstallRequested,
    /// A ride's import was acknowledged, under the same no-revision rule.
    RideAcknowledged,
}

impl ChangeKind {
    /// The wire outcome this change reports as (§10), for a consumer that speaks in those terms.
    pub const fn outcome(self) -> ObjectOutcome {
        match self {
            ChangeKind::Created | ChangeKind::Replaced => ObjectOutcome::Committed,
            ChangeKind::Deleted => ObjectOutcome::Deleted,
            ChangeKind::MetadataChanged => ObjectOutcome::MetadataChanged,
            ChangeKind::InstallRequested => ObjectOutcome::UpdateInstallRequested,
            ChangeKind::RideAcknowledged => ObjectOutcome::RideImported,
        }
    }

    /// Whether this change moved the repository revision.
    ///
    /// The two command outcomes change no head, so they move no revision — a repository whose
    /// revision advanced would tell every other consumer that something it can see has changed.
    pub const fn moves_revision(self) -> bool {
        !matches!(self, ChangeKind::InstallRequested | ChangeKind::RideAcknowledged)
    }
}

/// §4's commit event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitEvent {
    /// The store the commit landed in. A consumer that holds one from a card that has since been
    /// replaced can tell, because a replacement mints a new StoreId.
    pub store: StoreId,
    /// The repository that emitted it. There is exactly one repository per kind, so the kind *is*
    /// the repository identity and §4's two words name one field.
    pub kind: ObjectKind,
    /// The head it concerns, absent only for a commit that named no head.
    pub logical_object_id: Option<LogicalObjectId>,
    /// The repository revision after the commit. This is the catch-up cursor: a consumer that
    /// missed edges reads the current catalog at this revision and has lost nothing.
    pub revision: Revision,
    /// What the commit did.
    pub change: ChangeKind,
    /// The operation that committed it, so a consumer can tie the edge to work it started.
    pub operation: OperationId,
}

/// The retained revision and coalescing wake §4 describes, for one mounted store.
///
/// It is bounded by construction rather than by a policy: one slot per repository, newest wins.
#[derive(Debug, Clone)]
pub struct CommitLog {
    /// The newest event each repository emitted, retained after delivery.
    latest: [Option<CommitEvent>; REPOSITORIES],
    /// Which repositories a consumer has not yet been woken for, oldest first.
    pending: Vec<u8, REPOSITORIES>,
}

impl Default for CommitLog {
    fn default() -> Self {
        CommitLog::new()
    }
}

impl CommitLog {
    /// An empty log.
    pub const fn new() -> Self {
        CommitLog { latest: [None; REPOSITORIES], pending: Vec::new() }
    }

    /// The slot a kind occupies. Total, and stable: it is the kind's position in the registry.
    const fn slot(kind: ObjectKind) -> usize {
        match kind {
            ObjectKind::Route => 0,
            ObjectKind::Trip => 1,
            ObjectKind::Ride => 2,
            ObjectKind::Weather => 3,
            ObjectKind::VolumeManifest => 4,
            ObjectKind::UpdatePackage => 5,
        }
    }

    /// Records one commit, coalescing it onto whatever that repository last emitted.
    ///
    /// This is the whole of §4's "consecutive events for one repository may be coalesced into the
    /// latest Revision". A repository already waiting to be delivered is not queued twice, so the
    /// wake queue can never hold more entries than there are repositories and `record` never fails.
    pub fn record(&mut self, event: CommitEvent) {
        let slot = CommitLog::slot(event.kind);
        self.latest[slot] = Some(event);
        if !self.pending.contains(&(slot as u8)) {
            // Infallible: `pending` holds at most one entry per repository and the guard above is
            // what makes that true, so the push cannot be the one that overflows.
            let _ = self.pending.push(slot as u8);
        }
    }

    /// Takes the next repository's wake, oldest repository first.
    ///
    /// The event it returns is the *newest* that repository emitted, not the oldest outstanding one:
    /// the intermediate revisions are exactly what §4 permits skipping.
    pub fn take(&mut self) -> Option<CommitEvent> {
        if self.pending.is_empty() {
            return None;
        }
        let slot = usize::from(self.pending.remove(0));
        self.latest[slot]
    }

    /// The newest event a repository emitted, whether or not its wake has been taken.
    ///
    /// This is the "retained revision" half: a consumer that reconnects, or one that never
    /// subscribed, reads it and is caught up without an event ever having been delivered.
    pub fn latest(&self, kind: ObjectKind) -> Option<CommitEvent> {
        self.latest[CommitLog::slot(kind)]
    }

    /// How many repositories are waiting to wake a consumer. Never more than [`REPOSITORIES`].
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Whether any repository is waiting.
    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Forgets everything, for a store that is no longer the store these events came from.
    ///
    /// §3: "Changing StoreId changes intent identity", and a card replacement invalidates every
    /// client link — an event retained across one would name a revision in a store that no longer
    /// exists.
    pub fn clear(&mut self) {
        self.latest = [None; REPOSITORIES];
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STORE: StoreId = StoreId::new([0x2C; 16]);

    fn event(kind: ObjectKind, revision: u64, change: ChangeKind) -> CommitEvent {
        CommitEvent {
            store: STORE,
            kind,
            logical_object_id: Some(LogicalObjectId::new(7)),
            revision: Revision::new(revision),
            change,
            operation: OperationId::new([revision as u8; 16]),
        }
    }

    #[test]
    fn consecutive_events_for_one_repository_coalesce_into_the_latest_revision() {
        let mut log = CommitLog::new();
        log.record(event(ObjectKind::Route, 1, ChangeKind::Created));
        log.record(event(ObjectKind::Route, 2, ChangeKind::Replaced));
        log.record(event(ObjectKind::Route, 3, ChangeKind::MetadataChanged));

        assert_eq!(log.pending(), 1, "three route commits are one wake");
        let taken = log.take().expect("a wake");
        assert_eq!(taken.revision, Revision::new(3), "the wake carries the latest revision, not the first");
        assert_eq!(taken.change, ChangeKind::MetadataChanged);
        assert!(log.take().is_none(), "one repository, one wake");
    }

    #[test]
    fn the_retained_revision_outlives_the_wake() {
        let mut log = CommitLog::new();
        log.record(event(ObjectKind::Weather, 9, ChangeKind::Replaced));
        assert!(log.take().is_some());
        // §4: a consumer that missed the edge "obtains the identical catch-up from the recovered
        // repository Revision" — so the fact has to survive delivery, not be consumed by it.
        assert_eq!(log.latest(ObjectKind::Weather).map(|event| event.revision), Some(Revision::new(9)));
        assert_eq!(log.pending(), 0);
    }

    #[test]
    fn repositories_are_woken_independently_and_in_order() {
        let mut log = CommitLog::new();
        log.record(event(ObjectKind::Trip, 1, ChangeKind::Created));
        log.record(event(ObjectKind::Route, 2, ChangeKind::Created));
        log.record(event(ObjectKind::Trip, 3, ChangeKind::Replaced));

        assert_eq!(log.pending(), 2, "two repositories, two wakes, however many commits");
        let first = log.take().expect("trip first: it was queued first");
        assert_eq!((first.kind, first.revision), (ObjectKind::Trip, Revision::new(3)));
        let second = log.take().expect("then route");
        assert_eq!((second.kind, second.revision), (ObjectKind::Route, Revision::new(2)));
        assert!(log.take().is_none());
    }

    /// The bound is structural: there is nothing to overflow, so a flood cannot lose an edge that
    /// matters. Whatever a consumer reads after the flood is the state the flood produced.
    #[test]
    fn a_flood_cannot_overflow_the_queue_or_lose_the_state() {
        let mut log = CommitLog::new();
        for revision in 1..=1_000u64 {
            let kind = ObjectKind::ALL[(revision as usize) % ObjectKind::ALL.len()];
            log.record(event(kind, revision, ChangeKind::Replaced));
            assert!(log.pending() <= REPOSITORIES, "the wake queue is indexed by repository");
        }
        let mut woken = 0;
        while let Some(event) = log.take() {
            woken += 1;
            assert_eq!(
                log.latest(event.kind).map(|latest| latest.revision),
                Some(event.revision),
                "the wake carries what the repository actually stands at"
            );
        }
        assert_eq!(woken, REPOSITORIES, "every repository woke exactly once for a thousand commits");
    }

    #[test]
    fn a_change_kind_reports_the_wire_outcome_and_whether_a_revision_moved() {
        assert_eq!(ChangeKind::Created.outcome(), ObjectOutcome::Committed);
        assert_eq!(ChangeKind::Replaced.outcome(), ObjectOutcome::Committed);
        assert_eq!(ChangeKind::Deleted.outcome(), ObjectOutcome::Deleted);
        assert_eq!(ChangeKind::MetadataChanged.outcome(), ObjectOutcome::MetadataChanged);
        assert_eq!(ChangeKind::InstallRequested.outcome(), ObjectOutcome::UpdateInstallRequested);
        assert_eq!(ChangeKind::RideAcknowledged.outcome(), ObjectOutcome::RideImported);
        assert!(ChangeKind::Created.moves_revision());
        assert!(!ChangeKind::InstallRequested.moves_revision());
        assert!(!ChangeKind::RideAcknowledged.moves_revision());
    }

    #[test]
    fn a_replaced_store_forgets_every_event_it_held() {
        let mut log = CommitLog::new();
        log.record(event(ObjectKind::Route, 4, ChangeKind::Created));
        log.clear();
        assert_eq!(log.pending(), 0);
        assert!(log.latest(ObjectKind::Route).is_none());
    }
}
