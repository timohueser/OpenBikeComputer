//! The **Recorder** domain: the ride session and its persistence lifecycle (#1436, #1398 R2).
//!
//! [`RecorderMachine`] owns the ride identity, when a ride starts, when a checkpoint is owed, and
//! what "closed" means. The executor writes bytes and reports what happened — it never reconciles a
//! ride session out of several application fields, and it never announces a save by re-feeding a
//! catalog it does not own (#1433 §7.3).
//!
//! ## The rider names the close to its owner
//!
//! A screen calls [`request`](RecorderMachine::request) through `Ctx::recorder` as the gesture
//! happens, exactly as it names a plan to Navigator. There is no one-shot in
//! [`Activity`](crate::Activity) for a host to drain and no `Connections` row for the intent: the
//! request is with its owner before stage 1 of the pass that acts on it.
//!
//! ## The close is a verdict
//!
//! A [`Save`](RecorderIntent::Save) becomes a [`Finalize`](RecorderEffect::Finalize) effect, and the
//! session stays open until the executor answers it. A failed finalize is a
//! [`Failed`](RecorderOutcome::Failed) with a typed reason, not a silent discard: the object is
//! still on the store, so the honest state is the one a retry can still finish.
//!
//! **Where the samples live.** A track batch is bulk and never rides an effect. Recorder stages its
//! samples in its own bounded buffer and an [`Append`](RecorderEffect::Append) names *how many* are
//! ready; the executor drains that buffer during the permitted phase and reports how many it wrote.
//! Sample assembly itself is part 2 of the cutover (#1553); nothing here emits an `Append` yet.

use crate::breadcrumb::Breadcrumb;
use crate::device_core::{OperationToken, RecorderCapabilities, RecorderTag, TokenSource};
use crate::placement::define_placement_constructors;
use crate::weather::SpeedWindow;
use crate::CatalogObjectId;

/// How long a ride may go unjournalled. The cadence is the domain's, not an executor's: a board and
/// a host that disagreed about it would give the same ride two different recovery windows.
const CHECKPOINT_MS: u32 = 10_000;

/// The accumulator state that must cross a reset when a journaled ride is continued.
///
/// This is deliberately the raw integration state, not just the rounded footer summary: averages
/// need their numerators and denominators in order to merge post-reset samples without drift.
/// Position/elevation anchors are intentionally absent; a reboot is a sampling gap, so the first
/// post-boot fix and altitude re-anchor instead of booking movement across power-off time.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RideContinuation {
    pub ridden_m: f32,
    pub moving_m: f32,
    pub moving_s: f32,
    pub climb_m: f32,
    pub descent_m: f32,
    pub hr_ms_sum: u64,
    pub hr_ms: u32,
    pub max_hr: u16,
    pub power_ms_sum: u64,
    pub power_ms: u32,
    pub max_power: u16,
    pub cadence_ms_sum: u64,
    pub cadence_ms: u32,
}

/// What a store did when an executor asked it to close the open ride.
///
/// Three states, because they mean three different things to Recorder and only one of them is a
/// retry. A store that could not tell "there was nothing to close" from "the close failed" would
/// put the domain in a retry loop against an object that does not exist — which is what happens
/// when a start the card refused is followed by the rider's Save.
///
/// Shared by both executors rather than each inventing its own: the board's flat recorder and the
/// hosts' `TrackRepository` answer this, and `serve_recorder` maps it to the outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RideClose {
    /// The store committed the ride under this identity.
    Committed(CatalogObjectId),
    /// There was no open ride. The goal state holds, so the close is over — it simply saved
    /// nothing, because there was never an object to save.
    Nothing,
    /// The close did not happen and **the ride is still there**. Recorder re-offers the same one.
    Failed,
}

/// What the rider asks of the ride recorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderIntent {
    /// Begin recording a ride.
    Start,
    /// Close the open ride and keep it as a durable ride object.
    Save,
    /// Close the open ride and throw it away.
    Discard,
}

/// One bounded physical recording operation, carrying the [`OperationToken`] Recorder issued.
///
/// There is deliberately no `Start` effect: starting is a Recorder state change, and the first
/// physical work a ride causes is its first [`Append`](RecorderEffect::Append).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderEffect {
    /// Write the `samples` points Recorder has staged.
    Append { token: OperationToken<RecorderTag>, samples: u16 },
    /// Make the ride recoverable across a power loss up to this point.
    Checkpoint { token: OperationToken<RecorderTag> },
    /// Close the ride into a durable ride object.
    Finalize { token: OperationToken<RecorderTag> },
    /// Delete the open ride and its journal.
    Discard { token: OperationToken<RecorderTag> },
}

impl RecorderEffect {
    /// The operation this effect belongs to.
    pub fn token(&self) -> OperationToken<RecorderTag> {
        match self {
            RecorderEffect::Append { token, .. }
            | RecorderEffect::Checkpoint { token }
            | RecorderEffect::Finalize { token }
            | RecorderEffect::Discard { token } => *token,
        }
    }
}

/// Why a recording operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderError {
    /// The medium refused or failed the write. Recorder keeps the samples staged and retries; the
    /// rider sees the recording warning rather than a lost ride.
    Write,
    /// No writable store is mounted — recording cannot proceed at all.
    NoStore,
}

/// The result of one [`RecorderEffect`].
///
/// A failed [`Finalize`](RecorderEffect::Finalize) is a [`Failed`](RecorderOutcome::Failed) with a
/// typed reason, not a silent discard: the epic's "a ride finalize fails after its last checkpoint"
/// trace depends on the ride still existing afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderOutcome {
    /// `samples` staged points reached the medium.
    Appended { token: OperationToken<RecorderTag>, samples: u16 },
    /// The ride is recoverable up to the checkpoint.
    Checkpointed { token: OperationToken<RecorderTag> },
    /// The ride is closed and committed under `ride`.
    Finalized { token: OperationToken<RecorderTag>, ride: CatalogObjectId },
    /// The open ride is gone.
    Discarded { token: OperationToken<RecorderTag> },
    /// The operation failed.
    Failed { token: OperationToken<RecorderTag>, error: RecorderError },
    /// The executor abandoned the operation without completing it.
    Cancelled { token: OperationToken<RecorderTag> },
}

impl RecorderOutcome {
    /// The operation this outcome answers.
    pub fn token(&self) -> OperationToken<RecorderTag> {
        match self {
            RecorderOutcome::Appended { token, .. }
            | RecorderOutcome::Checkpointed { token }
            | RecorderOutcome::Finalized { token, .. }
            | RecorderOutcome::Discarded { token }
            | RecorderOutcome::Failed { token, .. }
            | RecorderOutcome::Cancelled { token } => *token,
        }
    }
}

/// What [`advance`](RecorderMachine::advance) did with the rider's
/// [`Start`](RecorderIntent::Start) this pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecorderAdvance {
    /// No start was pending, or one is pending and nothing about it changed.
    Nothing,
    /// A session opened.
    Opened(SessionStart),
    /// The device cannot record, so no session opened — **and the request was kept**, exactly as a
    /// close is. A rider request is never destroyed by a device that cannot serve it yet.
    ///
    /// Reported **once**, on the pass that first refuses it. The rider is told, because a ride they
    /// believe is recording and is not is the failure this exists to prevent: they would ride it,
    /// end it, and find nothing saved with nothing having said so.
    Refused,
}

/// A ride session opened this pass, and what starts fresh with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionStart {
    /// A fresh ride: the totals, any planned detour and the trail all restart from zero.
    Fresh,
    /// A recovered ride continues: the restored totals stand and only the trail restarts.
    Recovered,
}

/// What one [`RecorderOutcome`] means to the rest of the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecorderVerdict {
    /// Nothing the app must act on — a checkpoint landed, or the answer was stale.
    Nothing,
    /// The ride closed and the store now holds it under `ride`.
    Saved(CatalogObjectId),
    /// The ride closed and its bytes are gone.
    Dropped,
    /// The operation failed. The ride is untouched and the rider must be told.
    Failed,
}

/// The **one owner** of the ride lifecycle (#1398 R1/R2): the session identity and its monotonic
/// source, whether a ride is open, the rider's undelivered request, the checkpoint deadline, the
/// boot-recovery decision, and the two per-session buffers a new ride restarts (the travelled
/// breadcrumb and the pace window).
///
/// It produces exactly one thing: a bounded [`RecorderEffect`], offered by stage 7 while
/// [`RecorderCapabilities::record`] holds. Nothing else in the app decides that a ride is open or
/// closed, and no executor decides it either.
pub struct RecorderMachine {
    /// The open ride's session id, or `None` when no ride is open. A ride the rider has closed is
    /// still open here until the executor's verdict lands — a finalize that failed closed nothing.
    session: Option<u32>,
    /// Monotonic id source for [`session`](Self::session); only increments, so a new session is
    /// never mistaken for the one it replaced.
    seq: u32,
    /// The map-plane instant the last checkpoint was issued at. The deadline is
    /// [`CHECKPOINT_MS`] past it.
    last_checkpoint_ms: u32,
    /// The token source for the one recording operation that may be in flight.
    ops: TokenSource<RecorderTag>,
    /// The operation currently with the executor, so a failure re-arms the right thing.
    inflight: Option<InFlight>,
    /// The rider's undelivered request. A [`Start`](RecorderIntent::Start) is consumed by the stage
    /// that opens the session; a close is **kept** until the executor confirms it, so a failed or
    /// refused close re-offers rather than evaporating.
    pending: Option<RecorderIntent>,
    /// The next session continues a recovered journal, so its restored totals must survive the
    /// start. Armed by [`continue_recovered`](Self::continue_recovered) and spent by the open.
    resume_next: bool,
    /// "Save & start new" is two rider decisions in one gesture: the open ride closes and a fresh
    /// one opens behind it. They cannot be one intent — the new ride may not open until the store
    /// has answered for the old one, or the two would share a session.
    restart_after_close: bool,
    /// The rider has been told that this device cannot record. One card per **ask**, not one per
    /// pass: the request stays pending, so without this the warning would re-raise for ever — and
    /// [`request`](Self::request) clears it, so asking again is answered again.
    refusal_told: bool,
    /// A failed checkpoint owes its retry now rather than at the next deadline: the storage journal
    /// keeps the failed append staged and refuses further samples until the exact same write lands.
    checkpoint_owed: bool,
    /// Whether a boot-recovered ride has already been put to the rider this boot. A recorder may
    /// report the same resumable object every pass; the rider sees one decision card.
    recovery_offered: bool,
    /// The executor holds a recovered ride the rider has not decided about yet. It belongs to no
    /// session — that is what recovery *is* — so it is what lets a `Discard` become an effect with
    /// no session open: the object is on the store either way, and refusing to act on it would
    /// strand it there for the rest of the card's life.
    recovered_held: bool,
    /// The travelled-path breadcrumb (RAM, bounded), fed each logged fix and drawn on the Map.
    /// Per-session: a new ride starts with an empty trail.
    pub(crate) breadcrumb: Breadcrumb,
    /// The bounded recent moving-speed window feeding the weather ride projection (WX12). Also
    /// per-session — a new ride is a new pace.
    pub(crate) speed_win: SpeedWindow,
}

/// Which operation is with the executor. The outcome carries a token, not a subject, so this is how
/// a [`Failed`](RecorderOutcome::Failed) knows what to re-arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InFlight {
    Checkpoint,
    Close,
}

impl RecorderMachine {
    define_placement_constructors!(
        /// The boot state: no ride open, nothing requested, no recovery decision made.
        pub(crate) fn new();
        /// Initialize `slot` **in place** to the [`new`](RecorderMachine::new) state — the
        /// placement path the firmware boots through (the breadcrumb is KB-scale; nothing here may
        /// form a by-value `RecorderMachine` on the stack).
        pub(crate) unsafe fn init_in_place;
        fields {
            session: None,
            seq: 0,
            last_checkpoint_ms: 0,
            ops: TokenSource::new(),
            inflight: None,
            pending: None,
            resume_next: false,
            refusal_told: false,
            restart_after_close: false,
            checkpoint_owed: false,
            recovery_offered: false,
            recovered_held: false,
            breadcrumb: Breadcrumb::new(),
            speed_win: SpeedWindow::new(),
        }
    );

    /// Name what the rider wants of the recorder. The one door: a screen calls it as the gesture
    /// happens, through `Ctx::recorder`.
    ///
    /// A second request before the first is acted on replaces it, because both are the same rider
    /// saying the same kind of thing about the same ride — and the newer one is what they meant.
    /// Replacing the [`Save`](RecorderIntent::Save) that a "save & start new" armed disarms the
    /// restart with it: the new ride belonged to *that* close, and a rider who then asks to discard
    /// is not asking for a fresh ride behind it.
    pub fn request(&mut self, intent: RecorderIntent) {
        if intent != RecorderIntent::Save {
            self.restart_after_close = false;
        }
        // A fresh ask deserves a fresh answer: a rider who dismissed the recording warning and
        // pressed START again is told again, rather than met with the silence the latch exists to
        // stop between passes.
        self.refusal_told = false;
        self.pending = Some(intent);
    }

    /// "Save & start new" (the route-swap prompt): close the open ride, then open a fresh one the
    /// moment the store confirms the close.
    pub fn save_and_restart(&mut self) {
        self.request(RecorderIntent::Save);
        self.restart_after_close = true;
    }

    /// The rider continues the ride the executor recovered at boot. The session it opens keeps the
    /// restored accumulators instead of applying the fresh-ride reset.
    pub fn continue_recovered(&mut self) {
        self.resume_next = true;
        self.request(RecorderIntent::Start);
    }

    /// The open ride's session id, or `None` when no ride is open — the level an executor keys its
    /// ride log on.
    pub fn session(&self) -> Option<u32> {
        self.session
    }

    /// Whether a ride is open (recording, paused, or closing).
    pub fn recording(&self) -> bool {
        self.session.is_some()
    }

    /// Put a boot-recovered ride to the rider, once per boot. `false` means the decision was already
    /// offered or a ride is already open — recovery is a boot decision and can never replace a live
    /// session.
    pub(crate) fn offer_recovery(&mut self) -> bool {
        if self.recovery_offered || self.session.is_some() {
            return false;
        }
        self.recovery_offered = true;
        self.recovered_held = true;
        true
    }

    /// Advance the domain one pass: turn the rider's [`Start`](RecorderIntent::Start) into a session.
    ///
    /// Starting is a state change and not an effect, so it happens here rather than in
    /// [`next_effect`](Self::next_effect). It is gated on the capability, because a ride with
    /// nowhere to put it is not a ride.
    ///
    /// A refused start is **kept**, not thrown away. That is the same rule the close follows, and
    /// for the same reason: the domain does not destroy a rider request it cannot serve yet, and a
    /// device whose card mounts a pass later opens the ride the rider actually asked for. What the
    /// rider must not get is silence — see [`Refused`](RecorderAdvance::Refused).
    pub(crate) fn advance(&mut self, caps: RecorderCapabilities) -> RecorderAdvance {
        if !matches!(self.pending, Some(RecorderIntent::Start)) {
            return RecorderAdvance::Nothing;
        }
        if !caps.record {
            // Kept — and `resume_next` with it, so a refused Continue does not burn the recovered
            // ride's continuation edge on a pass that opened nothing.
            if core::mem::replace(&mut self.refusal_told, true) {
                return RecorderAdvance::Nothing;
            }
            return RecorderAdvance::Refused;
        }
        self.pending = None;
        self.refusal_told = false;
        let resume = core::mem::take(&mut self.resume_next);
        self.seq = self.seq.wrapping_add(1);
        self.session = Some(self.seq);
        self.checkpoint_owed = false;
        self.recovered_held = false; // adopted by this session, or never there to begin with
        RecorderAdvance::Opened(if resume { SessionStart::Recovered } else { SessionStart::Fresh })
    }

    /// The ride object an executor owes: the open session's id, unless `opened` already names it.
    ///
    /// **The id, not a boolean, and that is the whole point.** An executor that served a close at
    /// the top of its iteration has not yet run the pass that applies the verdict, so it still sees
    /// an open session here; a boolean "there is a session and no object" cannot tell that apart
    /// from "the start failed, retry", and would open a second ride object under the closing ride's
    /// identity. Naming the id makes the two distinguishable without the executor knowing anything
    /// about the pass order.
    ///
    /// `opened` is what the executor has already opened an object for; it is never cleared by a
    /// close, because a session that has been served is served whatever became of its object.
    pub fn object_owed(&self, opened: Option<u32>) -> Option<u32> {
        match self.session {
            Some(id) if opened != Some(id) => Some(id),
            _ => None,
        }
    }

    /// The one bounded operation this pass may carry, or `None`.
    ///
    /// Everything physical is refused without a writable store — this is
    /// [`RecorderCapabilities::record`]'s reader. One operation at a time, and **the close outranks
    /// the cadence**: a ride the rider has ended must not owe another checkpoint first.
    ///
    /// A close is not consumed here. It stays [`pending`](Self::pending) until the executor's
    /// verdict retires it, so a refused slot, a busy operation or a failed write all re-offer it
    /// instead of destroying the rider's request.
    pub(crate) fn next_effect(&mut self, caps: RecorderCapabilities, now_ms: u32) -> Option<RecorderEffect> {
        if !caps.record || self.inflight.is_some() {
            return None;
        }
        if self.session.is_none() && !self.recovered_held {
            return None; // nothing open and nothing recovered — there is no ride to act on
        }
        match self.pending {
            Some(RecorderIntent::Save) => {
                self.inflight = Some(InFlight::Close);
                Some(RecorderEffect::Finalize { token: self.ops.issue() })
            }
            Some(RecorderIntent::Discard) => {
                self.inflight = Some(InFlight::Close);
                Some(RecorderEffect::Discard { token: self.ops.issue() })
            }
            // `Start` is spent by `advance`; a stale one here would open nothing.
            Some(RecorderIntent::Start) | None => {
                if !self.checkpoint_due(now_ms) {
                    return None;
                }
                self.last_checkpoint_ms = now_ms;
                self.checkpoint_owed = false;
                self.inflight = Some(InFlight::Checkpoint);
                Some(RecorderEffect::Checkpoint { token: self.ops.issue() })
            }
        }
    }

    /// Consume the answer to a [`RecorderEffect`] and say what it means to the app.
    ///
    /// A stale token — a superseded operation, or a repeat of one already accounted for — changes
    /// nothing at all. That is what stops a late answer from closing a session that has since been
    /// replaced.
    pub(crate) fn apply_outcome(&mut self, outcome: RecorderOutcome) -> RecorderVerdict {
        if !self.ops.is_current(outcome.token()) {
            return RecorderVerdict::Nothing;
        }
        self.ops.invalidate(); // terminal: a repeat of this outcome is no longer current
        let was = self.inflight.take();
        match outcome {
            RecorderOutcome::Finalized { ride, .. } => {
                self.close();
                RecorderVerdict::Saved(ride)
            }
            RecorderOutcome::Discarded { .. } => {
                self.close();
                RecorderVerdict::Dropped
            }
            RecorderOutcome::Failed { error, .. } => {
                // A failed journal write keeps its staged append: the retry has to be the same
                // write, so it is owed now rather than at the next deadline. Nothing is owed for a
                // missing store — the capability gate above is what withholds the retry there.
                if was == Some(InFlight::Checkpoint) && error == RecorderError::Write {
                    self.checkpoint_owed = true;
                }
                // A close stays pending, so it re-offers: the ride is still on the store.
                RecorderVerdict::Failed
            }
            RecorderOutcome::Cancelled { .. } => {
                if was == Some(InFlight::Checkpoint) {
                    self.checkpoint_owed = true;
                }
                RecorderVerdict::Nothing
            }
            RecorderOutcome::Appended { .. } | RecorderOutcome::Checkpointed { .. } => RecorderVerdict::Nothing,
        }
    }

    /// Forget the previous ride's trail and pace — the two per-session buffers, cleared together
    /// because a new ride restarts both.
    pub(crate) fn restart_trail(&mut self) {
        self.breadcrumb.clear();
        self.speed_win.clear();
    }

    /// Whether a journal checkpoint is owed: a failed one is owed at once, otherwise the deadline.
    fn checkpoint_due(&self, now_ms: u32) -> bool {
        self.checkpoint_owed || now_ms.wrapping_sub(self.last_checkpoint_ms) >= CHECKPOINT_MS
    }

    /// The ride is over: drop the identity and any owed cadence.
    fn close(&mut self) {
        self.session = None;
        self.checkpoint_owed = false;
        self.recovered_held = false;
        // The "save & start new" second half, if the rider asked for one. Stage 1 lands the verdict
        // and stage 7 opens the new ride, so both halves of the gesture complete in one pass.
        self.pending = core::mem::take(&mut self.restart_after_close).then_some(RecorderIntent::Start);
    }
}

#[cfg(test)]
impl RecorderMachine {
    /// Open a session directly — the screen suites' stand-in for stage 7 over a mounted store.
    pub(crate) fn test_open(&mut self) {
        self.request(RecorderIntent::Start);
        let opened = self.advance(RecorderCapabilities { record: true });
        assert!(matches!(opened, RecorderAdvance::Opened(_)), "a mounted store admits the ride: {opened:?}");
    }

    /// Close the open session directly — the stand-in for the store's own verdict.
    pub(crate) fn test_close(&mut self) {
        self.pending = None;
        self.close();
    }

    /// The rider's undelivered request, taken — what a screen suite asserts a gesture named.
    pub(crate) fn test_take_intent(&mut self) -> Option<RecorderIntent> {
        self.pending.take()
    }

    /// Assert the [`new`](RecorderMachine::new) boot state, field by field. The destructure is
    /// exhaustive, so a field added to the plan must state its boot value here too.
    pub(crate) fn assert_boot_state(&self) {
        let RecorderMachine {
            session,
            seq,
            last_checkpoint_ms,
            ops: _,
            inflight,
            pending,
            resume_next,
            refusal_told,
            restart_after_close,
            checkpoint_owed,
            recovery_offered,
            recovered_held,
            breadcrumb,
            speed_win,
        } = self;
        assert!(session.is_none() && *seq == 0, "no ride has ever been open");
        assert_eq!(*last_checkpoint_ms, 0, "no checkpoint has been issued");
        assert!(inflight.is_none() && pending.is_none(), "nothing requested, nothing in flight");
        assert!(!*resume_next && !*restart_after_close, "no continuation and no restart armed");
        assert!(!*refusal_told, "the rider has not been refused a ride");
        assert!(!*checkpoint_owed, "no checkpoint owed");
        assert!(!*recovery_offered && !*recovered_held, "no recovered ride offered this boot");
        assert!(breadcrumb.is_empty(), "no trail");
        assert!(speed_win.median_cms().is_none(), "no moving speeds recorded");
    }
}

// Layout tripwires: a recorder message is a token, a count, or a ride identity — never a batch.
const _: () = assert!(core::mem::size_of::<RecorderIntent>() <= 1, "three fieldless requests");
const _: () = assert!(core::mem::size_of::<RecorderEffect>() <= 8, "a token and a sample count");
const _: () = assert!(core::mem::size_of::<RecorderOutcome>() <= 16, "a token and a ride identity");
const _: () = assert!(core::mem::size_of::<RecorderError>() <= 1, "a verdict, not a report");

#[cfg(test)]
mod tests {
    use super::*;

    const CAN_RECORD: RecorderCapabilities = RecorderCapabilities { record: true };
    const NO_STORE: RecorderCapabilities = RecorderCapabilities { record: false };

    fn recording() -> RecorderMachine {
        let mut rec = RecorderMachine::new();
        rec.request(RecorderIntent::Start);
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Opened(SessionStart::Fresh));
        rec
    }

    /// A ride needs somewhere to put it. Without a writable store no session opens, the rider is
    /// told **once**, and the request is **kept** — a rider request is never destroyed by a device
    /// that cannot serve it yet, which is the rule the close already follows.
    #[test]
    fn recording_is_refused_without_a_writable_store() {
        let mut rec = RecorderMachine::new();
        rec.request(RecorderIntent::Start);
        assert_eq!(rec.advance(NO_STORE), RecorderAdvance::Refused, "no store, no ride — and say so");
        assert!(!rec.recording());
        assert!(rec.next_effect(NO_STORE, 60_000).is_none(), "and nothing physical is offered either");
        assert_eq!(rec.advance(NO_STORE), RecorderAdvance::Nothing, "one card per ask, not one per pass");

        // …but a rider who asks again is answered again, rather than met with silence.
        rec.request(RecorderIntent::Start);
        assert_eq!(rec.advance(NO_STORE), RecorderAdvance::Refused, "a fresh ask gets a fresh answer");

        // The card mounts. The request the rider made is still theirs, and it opens the ride.
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Opened(SessionStart::Fresh), "the kept request opens it");
        assert!(rec.recording());
    }

    /// A refused Continue must not burn the recovered ride's continuation edge: the rider decided
    /// once, and a pass that opened nothing cannot spend that decision.
    #[test]
    fn a_refused_continue_keeps_the_continuation_edge() {
        let mut rec = RecorderMachine::new();
        assert!(rec.offer_recovery());
        rec.continue_recovered();
        assert_eq!(rec.advance(NO_STORE), RecorderAdvance::Refused);
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Opened(SessionStart::Recovered), "still a continuation");
    }

    /// The ride object an executor owes is named by **id**, so a close served before its verdict is
    /// applied cannot look like a start that failed.
    ///
    /// This is the board's ordering: it serves the effect at the top of an iteration and runs the
    /// pass at the end of it, so `session()` is still `Some(N)` while the object is already gone.
    #[test]
    fn a_close_answered_before_its_verdict_owes_no_second_object() {
        let mut rec = recording();
        let first = rec.session().expect("a ride is open");
        assert_eq!(rec.object_owed(None), Some(first), "an unopened session owes its object");
        let opened = Some(first);
        assert_eq!(rec.object_owed(opened), None, "and an opened one owes nothing");

        // The close is served; the verdict has not been applied yet.
        rec.request(RecorderIntent::Save);
        let effect = rec.next_effect(CAN_RECORD, 1).expect("a finalize");
        assert_eq!(rec.object_owed(opened), None, "the closing ride must not be opened a second time");
        rec.apply_outcome(RecorderOutcome::Finalized { token: effect.token(), ride: 9 });
        assert_eq!(rec.object_owed(opened), None, "and neither must the closed one");

        // The next ride is a different identity, and it owes its own object.
        rec.request(RecorderIntent::Start);
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Opened(SessionStart::Fresh));
        assert_eq!(rec.object_owed(opened), rec.session(), "a new ride owes a new object");
    }

    /// A "save & start new" the rider then turns into a Discard opens no ride behind it.
    #[test]
    fn a_discard_after_an_armed_restart_opens_nothing_behind_it() {
        let mut rec = recording();
        rec.save_and_restart();
        rec.request(RecorderIntent::Discard); // the rider changed their mind
        let effect = rec.next_effect(CAN_RECORD, 1).expect("a discard");
        assert!(matches!(effect, RecorderEffect::Discard { .. }));
        rec.apply_outcome(RecorderOutcome::Discarded { token: effect.token() });
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Nothing, "no ride follows a discard");
        assert!(!rec.recording());
    }

    /// The rider's close becomes the finalize in the very pass that offers it, and the session stays
    /// open until the executor answers.
    #[test]
    fn the_close_becomes_a_finalize_and_the_session_survives_until_it_is_answered() {
        let mut rec = recording();
        rec.request(RecorderIntent::Save);
        let effect = rec.next_effect(CAN_RECORD, 1).expect("the close outranks the cadence");
        assert!(matches!(effect, RecorderEffect::Finalize { .. }));
        assert!(rec.recording(), "the ride is open until the store says otherwise");

        let verdict = rec.apply_outcome(RecorderOutcome::Finalized { token: effect.token(), ride: 42 });
        assert_eq!(verdict, RecorderVerdict::Saved(42));
        assert!(!rec.recording());
    }

    /// A failed finalize leaves the ride open and re-offers itself: the object is still on the store
    /// and the rider never asked for it to disappear.
    #[test]
    fn a_failed_finalize_keeps_the_ride_and_retries() {
        let mut rec = recording();
        rec.request(RecorderIntent::Save);
        let effect = rec.next_effect(CAN_RECORD, 1).expect("a finalize");
        let verdict = rec.apply_outcome(RecorderOutcome::Failed { token: effect.token(), error: RecorderError::Write });
        assert_eq!(verdict, RecorderVerdict::Failed);
        assert!(rec.recording(), "the ride the store still holds is still open");
        assert!(matches!(rec.next_effect(CAN_RECORD, 2), Some(RecorderEffect::Finalize { .. })), "and it retries");
    }

    /// One operation at a time, and a close offered into a pass that cannot carry it is kept, not
    /// destroyed.
    #[test]
    fn a_close_offered_while_the_slot_is_busy_is_not_lost() {
        let mut rec = recording();
        let checkpoint = rec.next_effect(CAN_RECORD, CHECKPOINT_MS).expect("the cadence came due");
        assert!(matches!(checkpoint, RecorderEffect::Checkpoint { .. }));

        rec.request(RecorderIntent::Save);
        assert!(rec.next_effect(CAN_RECORD, CHECKPOINT_MS + 1).is_none(), "one operation at a time");
        rec.apply_outcome(RecorderOutcome::Checkpointed { token: checkpoint.token() });
        assert!(
            matches!(rec.next_effect(CAN_RECORD, CHECKPOINT_MS + 2), Some(RecorderEffect::Finalize { .. })),
            "the rider's Save survived the busy pass"
        );
    }

    /// "Save & start new": the fresh ride opens only once the store has answered for the old one.
    #[test]
    fn save_and_restart_opens_the_new_ride_behind_the_close() {
        let mut rec = recording();
        let first = rec.session();
        rec.save_and_restart();
        let effect = rec.next_effect(CAN_RECORD, 1).expect("the close goes first");
        assert!(matches!(effect, RecorderEffect::Finalize { .. }));
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Nothing, "the new ride waits for the old one's verdict");

        rec.apply_outcome(RecorderOutcome::Finalized { token: effect.token(), ride: 5 });
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Opened(SessionStart::Fresh));
        assert!(rec.recording() && rec.session() != first, "a fresh ride, with its own identity");
    }

    /// A superseded answer changes nothing — the guard that stops a late verdict closing a ride that
    /// has already been replaced.
    #[test]
    fn a_stale_recorder_outcome_changes_nothing() {
        let mut rec = recording();
        rec.request(RecorderIntent::Save);
        let stale = rec.next_effect(CAN_RECORD, 1).expect("a finalize");
        rec.apply_outcome(RecorderOutcome::Finalized { token: stale.token(), ride: 7 });

        // A second ride, and the first one's answer arrives late.
        rec.request(RecorderIntent::Start);
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Opened(SessionStart::Fresh));
        assert_eq!(
            rec.apply_outcome(RecorderOutcome::Finalized { token: stale.token(), ride: 7 }),
            RecorderVerdict::Nothing
        );
        assert!(rec.recording(), "the newer ride is untouched");
    }

    /// The checkpoint cadence is the domain's: due on the deadline, and owed at once after a failed
    /// journal write.
    #[test]
    fn the_checkpoint_cadence_is_the_deadline_and_a_failed_write_owes_one_now() {
        let mut rec = recording();
        assert!(rec.next_effect(CAN_RECORD, CHECKPOINT_MS - 1).is_none(), "not yet due");
        let first = rec.next_effect(CAN_RECORD, CHECKPOINT_MS).expect("due");
        rec.apply_outcome(RecorderOutcome::Checkpointed { token: first.token() });
        assert!(rec.next_effect(CAN_RECORD, CHECKPOINT_MS + 1).is_none(), "the deadline moved with it");

        let second = rec.next_effect(CAN_RECORD, 2 * CHECKPOINT_MS).expect("due again");
        rec.apply_outcome(RecorderOutcome::Failed { token: second.token(), error: RecorderError::Write });
        assert!(
            matches!(rec.next_effect(CAN_RECORD, 2 * CHECKPOINT_MS + 1), Some(RecorderEffect::Checkpoint { .. })),
            "a blocked journal owes the same write now, not in ten seconds"
        );
    }

    /// The rider discards the ride the executor recovered at boot. It belongs to no session, and it
    /// still has to leave the store.
    #[test]
    fn a_recovered_ride_can_be_discarded_without_a_session() {
        let mut rec = RecorderMachine::new();
        assert!(rec.offer_recovery());
        rec.request(RecorderIntent::Discard);
        let effect = rec.next_effect(CAN_RECORD, 1).expect("the recovered object is discardable");
        assert!(matches!(effect, RecorderEffect::Discard { .. }));
        assert_eq!(rec.apply_outcome(RecorderOutcome::Discarded { token: effect.token() }), RecorderVerdict::Dropped);
        assert!(rec.next_effect(CAN_RECORD, 2).is_none(), "and nothing is left to act on");
    }

    /// A recovered ride continues without resetting its totals, and the continuation edge is spent
    /// once.
    #[test]
    fn a_recovered_ride_continues_without_resetting_its_totals() {
        let mut rec = RecorderMachine::new();
        assert!(rec.offer_recovery(), "the decision is put to the rider");
        assert!(!rec.offer_recovery(), "once per boot");

        rec.continue_recovered();
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Opened(SessionStart::Recovered));

        // The next ride after it is an ordinary one.
        rec.request(RecorderIntent::Discard);
        let effect = rec.next_effect(CAN_RECORD, 1).expect("a discard");
        rec.apply_outcome(RecorderOutcome::Discarded { token: effect.token() });
        rec.request(RecorderIntent::Start);
        assert_eq!(
            rec.advance(CAN_RECORD),
            RecorderAdvance::Opened(SessionStart::Fresh),
            "the continuation edge was one-shot"
        );
    }
}
