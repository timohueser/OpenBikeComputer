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
//! 1. **Coalescing is sound exactly when the revision carries the fact.** §4's justification is
//!    conditional and the condition matters: a consumer "that reads the current state loses nothing
//!    by skipping intermediate ones". That holds for every commit that *moved* the repository
//!    revision, because the state it reads afterwards is the state those commits produced. So those
//!    get one slot per repository, newest wins, and there is no capacity to exhaust.
//!
//!    It does **not** hold for the two commits that move no revision. `InstallUpdate` and
//!    `AcknowledgeRideImported` change no head — that is why [`ChangeKind::moves_revision`] is false
//!    for them — so a consumer reading "the current state" afterwards finds nothing recording that
//!    they happened. Coalescing one of those into a later revision edge would destroy the only
//!    notice of it that exists. They therefore get a **second slot per repository**, which an
//!    explicit [`take`](CommitLog::take) clears and nothing else overwrites. One slot is enough
//!    because a repository has at most one such change kind: the update repository's is
//!    `InstallRequested` and the ride repository's is `RideAcknowledged`, and no repository has both.
//! 2. **Delivery is a retained revision plus a wake, not a stream.** [`CommitLog::latest`] answers
//!    "what does this repository stand at" whenever it is asked, and stays answerable after the wake
//!    has been taken; [`CommitLog::take`] is only the edge that says a consumer has something to do.
//! 3. **Nothing here is durable.** A reboot loses the log. For a revision edge §4 says that costs
//!    nothing — the recovered repository Revision is the identical catch-up. For the two command
//!    edges the durable fact is elsewhere and is not this type's to keep: an install request lives
//!    in the boot handoff and an import acknowledgement in the ride head's own metadata, and a
//!    consumer that missed one across a reboot reads it there.
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

/// Which of a repository's two slots a wake names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Wake {
    /// The repository's index.
    slot: u8,
    /// True for the command slot, false for the revision slot.
    command: bool,
}

/// The retained revision and coalescing wake §4 describes, for one mounted store.
///
/// It is bounded by construction rather than by a policy: **two** slots per repository — one for the
/// revision-moving edges, which coalesce, and one for the single command edge that repository can
/// emit, which does not — so the wake queue is at most `2 × REPOSITORIES` and `record` can never
/// fail.
#[derive(Debug, Clone)]
pub struct CommitLog {
    /// The newest revision-moving event each repository emitted, retained after delivery.
    latest: [Option<CommitEvent>; REPOSITORIES],
    /// The undelivered command edge, which moves no revision and is therefore recorded nowhere a
    /// consumer could read it back. Cleared by delivery, not by a later commit.
    command: [Option<CommitEvent>; REPOSITORIES],
    /// Which slots a consumer has not yet been woken for, oldest first.
    pending: Vec<Wake, { REPOSITORIES * 2 }>,
}

impl Default for CommitLog {
    fn default() -> Self {
        CommitLog::new()
    }
}

impl CommitLog {
    /// An empty log.
    pub const fn new() -> Self {
        CommitLog { latest: [None; REPOSITORIES], command: [None; REPOSITORIES], pending: Vec::new() }
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

    /// Records one commit.
    ///
    /// A revision-moving commit coalesces onto whatever that repository last emitted — §4's
    /// "consecutive events for one repository may be coalesced into the latest Revision". A command
    /// commit does not: it lands in its own slot, because the revision a later commit leaves behind
    /// says nothing about whether an install was requested or an import acknowledged.
    ///
    /// A slot already waiting to be delivered is not queued twice, so the wake queue can never hold
    /// more than two entries per repository and `record` never fails.
    pub fn record(&mut self, event: CommitEvent) {
        let slot = CommitLog::slot(event.kind);
        let command = !event.change.moves_revision();
        if command {
            self.command[slot] = Some(event);
        } else {
            self.latest[slot] = Some(event);
        }
        let wake = Wake { slot: slot as u8, command };
        if !self.pending.contains(&wake) {
            // Infallible: `pending` holds at most one entry per (repository, slot) pair and the
            // guard above is what makes that true, so the push cannot be the one that overflows.
            let _ = self.pending.push(wake);
        }
    }

    /// Takes the next outstanding wake, oldest first.
    ///
    /// For a revision edge the event it returns is the *newest* that repository emitted, not the
    /// oldest outstanding one: the intermediate revisions are exactly what §4 permits skipping. For
    /// a command edge it is the edge itself, and taking it is what clears it.
    pub fn take(&mut self) -> Option<CommitEvent> {
        if self.pending.is_empty() {
            return None;
        }
        let wake = self.pending.remove(0);
        let slot = usize::from(wake.slot);
        if wake.command {
            self.command[slot].take()
        } else {
            self.latest[slot]
        }
    }

    /// The newest revision-moving event a repository emitted, whether or not its wake has been taken.
    ///
    /// This is the "retained revision" half: a consumer that reconnects, or one that never
    /// subscribed, reads it and is caught up without an event ever having been delivered. It is
    /// deliberately *not* the command slot — a command edge is retained only until it is delivered,
    /// because a consumer catching up on one reads the boot handoff or the ride head rather than
    /// this log.
    pub fn latest(&self, kind: ObjectKind) -> Option<CommitEvent> {
        self.latest[CommitLog::slot(kind)]
    }

    /// The undelivered command edge a repository is holding, if any.
    pub fn pending_command(&self, kind: ObjectKind) -> Option<CommitEvent> {
        self.command[CommitLog::slot(kind)]
    }

    /// How many wakes are outstanding. Never more than two per repository.
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
        self.command = [None; REPOSITORIES];
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

    /// **The edge coalescing must not eat.** An install request moves no revision, so nothing a
    /// consumer can read afterwards records that it happened — a later revision edge for the same
    /// repository must therefore not replace it.
    ///
    /// This is the sequence that proved the earlier "no such thing as a dropped edge" claim false.
    #[test]
    fn a_command_edge_is_not_coalesced_away_by_a_later_revision_edge() {
        let mut log = CommitLog::new();
        log.record(event(ObjectKind::UpdatePackage, 4, ChangeKind::InstallRequested));
        log.record(event(ObjectKind::UpdatePackage, 5, ChangeKind::Created));

        assert_eq!(log.pending(), 2, "two facts, two wakes, one repository");
        let first = log.take().expect("the install request, queued first");
        assert_eq!(first.change, ChangeKind::InstallRequested, "the command edge survived the revision edge");
        let second = log.take().expect("then the publication");
        assert_eq!((second.change, second.revision), (ChangeKind::Created, Revision::new(5)));
        assert!(log.take().is_none());

        // Taking the command edge is what clears it; the revision edge stays readable.
        assert!(log.pending_command(ObjectKind::UpdatePackage).is_none());
        assert_eq!(log.latest(ObjectKind::UpdatePackage).map(|latest| latest.revision), Some(Revision::new(5)));
    }

    /// The other order, and the ride repository's own command edge.
    #[test]
    fn a_revision_edge_does_not_hide_a_command_edge_that_follows_it() {
        let mut log = CommitLog::new();
        log.record(event(ObjectKind::Ride, 8, ChangeKind::Created));
        log.record(event(ObjectKind::Ride, 8, ChangeKind::RideAcknowledged));
        assert_eq!(log.pending(), 2);
        assert_eq!(log.take().map(|event| event.change), Some(ChangeKind::Created));
        assert_eq!(log.take().map(|event| event.change), Some(ChangeKind::RideAcknowledged));
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
            assert!(log.pending() <= REPOSITORIES * 2, "the wake queue is indexed by (repository, slot)");
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
