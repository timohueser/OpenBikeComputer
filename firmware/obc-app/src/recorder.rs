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
//!
//! **The ride's numbers are the ride's owner's.** Distance, moving time, climb and the per-ride
//! sensor summary accrue here (#1398 R1), so a session edge is the only thing that can zero them
//! and the footer is read from the machine that minted the close.

use obc_elevation::DeadBand;
use obc_map_scene::ground_dist_m;
use obc_ports::{Fix, TrackPoint};
use obc_route::RideStats;

use crate::altitude::AltitudeFusion;
use crate::breadcrumb::Breadcrumb;
use crate::device_core::{OperationToken, RecorderCapabilities, RecorderTag, TokenSource};
use crate::placement::define_placement_constructors;
use crate::weather::SpeedWindow;
use crate::CatalogObjectId;

/// How long a ride may go unjournalled. The cadence is the domain's, not an executor's: a board and
/// a host that disagreed about it would give the same ride two different recovery windows.
const CHECKPOINT_MS: u32 = 10_000;

/// How many assembled samples Recorder may hold before an executor has written them. Sized against
/// the executor's delta window (the board's `DELTA_SAMPLES`), so the domain never stages more than
/// one checkpoint interval's worth. An [`Append`](RecorderEffect::Append) is offered on every pass a
/// slot is free, so in practice this holds one.
const STAGED_SAMPLES: usize = 16;

/// A gap longer than this between fixes (s) is a GPS dropout, not real travel — skip the interval
/// so a reconnect doesn't book a straight-line jump across it.
const MAX_GAP_S: f32 = 10.0;
/// Implied speed above this (m/s ≈ 108 km/h) is a teleport / glitch (manual drag, GPS jump) — skip
/// the interval rather than crediting impossible distance.
const MAX_SPEED_MPS: f32 = 30.0;
/// Below this implied speed (m/s) the rider is stopped; don't count the time toward the moving
/// average, so red lights and rests don't drag Avg. Speed down.
const MOVING_MIN_MPS: f32 = 0.8;
/// A BLE sensor sample (HR / power / cadence) older than this (ms) is stale: the live accessors read
/// `None` and the summary stops accumulating it. A dropped strap must show `--` on the tile and
/// record *absent* into the log, never freeze its last value.
const SENSOR_STALE_MS: u32 = 5_000;

/// The wall-clock anchor a ride footer carries. It is what the *device* knows about the time of
/// day, not a ride accumulator, so the pass gives it to Recorder as it offers the operation slot —
/// which is what pairs the anchor with the samples that operation writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FooterClock {
    /// UTC seconds at [`anchor_ms`](Self::anchor_ms), or 0 with no trusted clock.
    pub unix_at_anchor: u32,
    /// The map-plane instant [`unix_at_anchor`](Self::unix_at_anchor) was read at.
    pub anchor_ms: u32,
    /// Whether the wall clock has a trusted source (GPS or BLE).
    pub trusted: bool,
}

/// What one fix did to the ride log: whether it was **logged** (fed the trail and staged a sample)
/// and whether it **starts a new track segment** — the first fix of a session, or the first after a
/// pause or a GPS gap, which is a fresh GPX `<trkseg>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Motion {
    log: bool,
    segment_start: bool,
}

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
    /// The assembled samples no executor has written yet. An [`Append`](RecorderEffect::Append)
    /// names how many are ready and the answer says how many reached the medium; the rest stay
    /// here. **The one sample queue in the device.**
    samples: heapless::Vec<TrackPoint, STAGED_SAMPLES>,
    /// The footer's wall-clock anchor, as of the pass that last offered an operation slot.
    clock: FooterClock,

    // ── the actually-ridden accumulators ────────────────────────────────────────────────────
    /// Distance actually pedalled (m) — the `done` stat. Counts **every** sane fix, including
    /// sub-threshold creep, so it is the true total covered.
    ridden_m: f32,
    /// Distance covered **while moving** (m): only fixes at or above [`MOVING_MIN_MPS`] — the
    /// numerator of Avg. Kept separate from [`ridden_m`](Self::ridden_m) so the average pairs
    /// moving distance with moving time (mixing them inflated Avg).
    moving_m: f32,
    /// Moving time (s), accumulated only above [`MOVING_MIN_MPS`] — denominator of Avg.
    moving_s: f32,
    /// Previous fix + its host timestamp, to integrate distance/time between ticks.
    last_fix: Option<Fix>,
    last_ms: Option<u32>,
    /// Dead-banded barometric climb — the `climbed` stat. The same hysteresis integrator the route
    /// converter uses, so an on-route ride lands near the route's precomputed ascent.
    climb: DeadBand<f32>,
    /// Latest barometric altitude (m), stamped onto each staged [`TrackPoint`]'s elevation.
    last_alt: Option<f32>,
    /// The map-referenced altimeter (EL8, epic #1068): the slow estimate of the barometer's absolute
    /// offset, fed one terrain sample per GPS fix. Corrects **what the Elevation tile shows** and
    /// nothing else — [`last_alt`](Self::last_alt), and therefore the recorded track and the climb
    /// dead-band, stays raw barometry on purpose (see [`crate::altitude`]). It lives here because it
    /// reads the same barometer, and [`reset_totals`](Self::reset_totals) leaves it alone: it is a
    /// calibration of the atmosphere, not a tally of the ride.
    altitude: AltitudeFusion,
    /// `true` when a dropped fix (GPS gap / teleport) left a hole, so the next staged sample starts
    /// a fresh track segment.
    segment_break: bool,

    // Live BLE sensor values (staleness-gated) — HR / power / cadence. Each holds the last sample +
    // the ride-clock ms it arrived; the `live_*` accessors return `None` once it is older than
    // [`SENSOR_STALE_MS`] so a dropped strap reads `--` rather than freezing.
    hr_last: Option<u16>,
    hr_at_ms: u32,
    power_last: Option<u16>,
    power_at_ms: u32,
    cadence_last: Option<u8>,
    cadence_at_ms: u32,
    /// The ride-clock ms of the most recent pass — the timebase samples record on. The
    /// `live_*_display` accessors judge staleness against it, so a stat tile rendered *after* the
    /// pass (the simulator's map-plane clock is wall time during a GPX replay) compares
    /// like-for-like with the record clock instead of spuriously blanking.
    sensor_now_ms: u32,

    // Per-ride sensor summary — time-weighted over **moving time** (the `avg_speed` discipline),
    // accruing only while a *fresh* value is present, in the same accepted-fix path as `moving_s`.
    // The weight is the interval's Δms (`_ms`); the sum is value×Δms (`_ms_sum`); the quotient is
    // the moving-time average. Reset with the session. No zones / smoothing / NP / TSS.
    hr_ms_sum: u64,
    hr_ms: u32,
    max_hr: u16,
    power_ms_sum: u64,
    power_ms: u32,
    max_power: u16,
    /// Σ(rpm × Δms) over cadence-present moving time, and its Δms denominator. Coasting-at-0 counts
    /// (a fresh `0`), strap-absent doesn't. No max — no consumer needs it.
    cadence_ms_sum: u64,
    cadence_ms: u32,
}

/// Which operation is with the executor. The outcome carries a token, not a subject, so this is how
/// a [`Failed`](RecorderOutcome::Failed) knows what to re-arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InFlight {
    Append,
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
            breadcrumb: Breadcrumb::new() => Breadcrumb::init_in_place,
            speed_win: SpeedWindow::new(),
            samples: heapless::Vec::new(),
            clock: FooterClock { unix_at_anchor: 0, anchor_ms: 0, trusted: false },
            ridden_m: 0.0,
            moving_m: 0.0,
            moving_s: 0.0,
            last_fix: None,
            last_ms: None,
            climb: DeadBand::new(),
            last_alt: None,
            altitude: AltitudeFusion::new(),
            segment_break: false,
            hr_last: None,
            hr_at_ms: 0,
            power_last: None,
            power_at_ms: 0,
            cadence_last: None,
            cadence_at_ms: 0,
            sensor_now_ms: 0,
            hr_ms_sum: 0,
            hr_ms: 0,
            max_hr: 0,
            power_ms_sum: 0,
            power_ms: 0,
            max_power: 0,
            cadence_ms_sum: 0,
            cadence_ms: 0,
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
        if intent != RecorderIntent::Start {
            // A rider who asks to close is not asking to continue a recovered ride. Leaving the
            // continuation armed would carry the discarded ride's restored totals into whatever
            // ride they start next.
            self.resume_next = false;
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

    /// The one bounded operation this pass may carry, or `None`. `clock` is the footer's wall-clock
    /// anchor as of this pass — stamped here so the figures an executor writes belong to the
    /// operation it is about to perform.
    ///
    /// Everything physical is refused without a writable store — this is
    /// [`RecorderCapabilities::record`]'s reader. One operation at a time, in a fixed rank:
    ///
    /// 1. **The close**, because a ride the rider has ended must not owe other work first.
    /// 2. **The checkpoint**, because an executor whose journal is blocked refuses samples until
    ///    the exact failed write lands — an append that outranked it would starve its own repair.
    /// 3. **The append**, offered on every remaining pass, which is nearly all of them.
    ///
    /// A close is not consumed here. It stays [`pending`](Self::pending) until the executor's
    /// verdict retires it, so a refused slot, a busy operation or a failed write all re-offer it
    /// instead of destroying the rider's request.
    pub(crate) fn next_effect(&mut self, caps: RecorderCapabilities, clock: FooterClock) -> Option<RecorderEffect> {
        self.clock = clock;
        // The anchor *is* this pass's map-plane instant, so the cadence reads its deadline from it
        // rather than being handed the same number twice.
        let now_ms = clock.anchor_ms;
        if !caps.record || self.inflight.is_some() {
            return None;
        }
        if self.session.is_none() && !self.recovered_held {
            return None; // nothing open and nothing recovered — there is no ride to act on
        }
        match self.pending {
            Some(RecorderIntent::Save) => {
                self.inflight = Some(InFlight::Close);
                return Some(RecorderEffect::Finalize { token: self.ops.issue() });
            }
            Some(RecorderIntent::Discard) => {
                self.inflight = Some(InFlight::Close);
                return Some(RecorderEffect::Discard { token: self.ops.issue() });
            }
            // `Start` is spent by `advance`; a stale one here would open nothing.
            Some(RecorderIntent::Start) | None => {}
        }
        if self.checkpoint_due(now_ms) {
            self.last_checkpoint_ms = now_ms;
            self.checkpoint_owed = false;
            self.inflight = Some(InFlight::Checkpoint);
            return Some(RecorderEffect::Checkpoint { token: self.ops.issue() });
        }
        if self.samples.is_empty() {
            return None;
        }
        self.inflight = Some(InFlight::Append);
        Some(RecorderEffect::Append { token: self.ops.issue(), samples: self.samples.len() as u16 })
    }

    /// The samples an executor serving an [`Append`](RecorderEffect::Append) must write, in order.
    /// It writes a prefix and says how long it was; there is no second copy of this anywhere.
    pub fn staged(&self) -> &[TrackPoint] {
        &self.samples
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
            // Only the prefix the medium actually took is retired. Everything behind it — and
            // everything a failure left untouched — stays staged for the next append, which is what
            // makes a partial write a delay rather than a hole in the ride log.
            RecorderOutcome::Appended { samples, .. } => {
                let taken = (samples as usize).min(self.samples.len());
                let keep = self.samples.len() - taken;
                self.samples.as_mut_slice().copy_within(taken.., 0);
                self.samples.truncate(keep);
                RecorderVerdict::Nothing
            }
            RecorderOutcome::Checkpointed { .. } => RecorderVerdict::Nothing,
        }
    }

    /// Forget the previous ride's per-session buffers: the trail, the pace window and any samples
    /// the ride that owned them never wrote. Cleared together because a new ride restarts all three.
    pub(crate) fn restart_buffers(&mut self) {
        self.breadcrumb.clear();
        self.speed_win.clear();
        self.samples.clear();
    }

    /// Whether a journal checkpoint is owed: a failed one is owed at once, otherwise the deadline.
    fn checkpoint_due(&self, now_ms: u32) -> bool {
        self.checkpoint_owed || now_ms.wrapping_sub(self.last_checkpoint_ms) >= CHECKPOINT_MS
    }

    /// The ride is over: drop the identity and any owed cadence.
    ///
    /// The continuation edge is **not** disarmed here, and that is deliberate (#1557 item 1): a
    /// close only ever reaches this through [`request`](Self::request), which already clears it for
    /// every intent other than `Start`. A second clear would be a line no test can fail against.
    fn close(&mut self) {
        self.session = None;
        self.checkpoint_owed = false;
        self.recovered_held = false;
        // The "save & start new" second half, if the rider asked for one. Stage 1 lands the verdict
        // and stage 7 opens the new ride, so both halves of the gesture complete in one pass.
        self.pending = core::mem::take(&mut self.restart_after_close).then_some(RecorderIntent::Start);
    }

    // ══ The world → the ride ═══════════════════════════════════════════════════════════════════

    /// Record the ride-clock ms of the current pass (see [`sensor_now_ms`](Self::sensor_now_ms)), so
    /// the `live_*_display` accessors judge freshness on the same clock samples record on.
    pub(crate) fn note_sensor_clock(&mut self, now_ms: u32) {
        self.sensor_now_ms = now_ms;
    }

    /// Store a fresh heart-rate sample, timestamped for the staleness gate.
    pub(crate) fn record_hr(&mut self, bpm: u16, now_ms: u32) {
        self.hr_last = Some(bpm);
        self.hr_at_ms = now_ms;
    }

    /// Store a fresh power sample, timestamped for the staleness gate.
    pub(crate) fn record_power(&mut self, watts: u16, now_ms: u32) {
        self.power_last = Some(watts);
        self.power_at_ms = now_ms;
    }

    /// Store a fresh cadence sample, timestamped for the staleness gate.
    pub(crate) fn record_cadence(&mut self, rpm: u8, now_ms: u32) {
        self.cadence_last = Some(rpm);
        self.cadence_at_ms = now_ms;
    }

    /// Integrate one barometric altitude sample into the climbed total, dead-banded so sensor noise
    /// doesn't inflate it. `riding` is false while paused or idle: the reference is dropped so an
    /// altitude change *during* the pause isn't booked on resume.
    pub(crate) fn record_altitude(&mut self, alt_m: f32, riding: bool) {
        // Reject non-finite samples (a baro driver hiccup): `+inf - ref = +inf >= DEADBAND` would
        // book *infinite* ascent, permanently poisoning `climbed`, and must never stamp a staged
        // elevation.
        if !alt_m.is_finite() {
            return;
        }
        // The latest altitude stamps staged samples regardless of mode; the climb dead-band below
        // only runs while riding.
        self.last_alt = Some(alt_m);
        if !riding {
            // Drop the reference so a height change during the pause isn't booked on resume; the
            // accumulated climb is kept.
            self.climb.pause();
            return;
        }
        self.climb.push(alt_m);
    }

    /// Feed one terrain sample taken at the current GPS fix into the map-referenced altimeter (EL8).
    /// Pairs it with the barometric reading from the **same pass**, so the residual is a
    /// like-for-like difference. A no-op before the first altimeter sample — with no barometer there
    /// is nothing to reference.
    pub(crate) fn record_map_elevation(&mut self, map_m: i16) {
        if let Some(baro) = self.last_alt {
            self.altitude.observe(f32::from(map_m), baro);
        }
    }

    /// Integrate one position fix into the ride, and stage the sample it produces.
    ///
    /// By the [`LocationSource`](obc_ports::LocationSource) contract this is called once per fresh
    /// GPS sample, so consecutive calls are a GPS period apart — the interval the gate below is
    /// sized for. `riding` is false while paused or idle: nothing accumulates, nothing is logged,
    /// and the anchor is dropped so resuming doesn't book the gap.
    ///
    /// Returns `true` when the staging buffer was full and the log lost a sample — the recording
    /// warning the pass raises, because a rider whose log has a hole must be told.
    pub(crate) fn record_fix(&mut self, fix: Fix, now_ms: u32, riding: bool) -> bool {
        let motion = self.integrate(fix, now_ms, riding);
        if !motion.log {
            return false;
        }
        self.breadcrumb.push(fix.lon, fix.lat);
        if self.session.is_none() {
            return false; // no ride open: the trail still grows, but nothing is being written
        }
        let point = TrackPoint {
            lon: fix.lon,
            lat: fix.lat,
            ele: self.last_alt.map_or(0, |a| a as i16),
            t_ms: now_ms,
            segment_start: motion.segment_start,
            // The freshest staleness-gated values (epic #707): a strap that is dropped or stale
            // records *absent*, never its frozen last value. `now_ms` is the timebase the samples
            // arrived on.
            hr: self.live_hr(now_ms).map(|b| b.min(u8::MAX as u16) as u8),
            cadence: self.live_cadence(now_ms),
            power: self.live_power(now_ms),
        };
        if self.samples.push(point).is_err() {
            // The buffer is a full checkpoint window deep and no executor has drained it. The
            // sample is gone, so the next one that lands starts a fresh segment rather than drawing
            // a line across the hole.
            self.segment_break = true;
            return true;
        }
        false
    }

    /// The distance/time half of [`record_fix`](Self::record_fix): the gates, the accumulators and
    /// the two decisions one fix produces.
    fn integrate(&mut self, fix: Fix, now_ms: u32, riding: bool) -> Motion {
        if !riding {
            self.last_fix = None;
            self.last_ms = None;
            return Motion::default();
        }
        let first = self.last_fix.is_none();
        let mut counted = false;
        if let (Some(prev), Some(prev_ms)) = (self.last_fix, self.last_ms) {
            let dt = now_ms.saturating_sub(prev_ms) as f32 / 1000.0;
            // A non-advancing clock (`dt <= 0`: two fixes stamped the same ms, or a source replaying
            // a stale fix) can't be integrated — `dist / dt` would manufacture an infinite implied
            // speed and reject the *next* real move as a teleport. Coalesce into the anchor instead:
            // advance `last_fix`/`last_ms`, log nothing, and do **not** arm a segment break (no time
            // or travel elapsed).
            if dt <= 0.0 {
                self.last_fix = Some(fix);
                self.last_ms = Some(now_ms);
                return Motion { log: false, segment_start: false };
            }
            let dist = ground_dist_m((prev.lon, prev.lat), (fix.lon, fix.lat));
            let implied = dist / dt;
            if dt < MAX_GAP_S && implied < MAX_SPEED_MPS {
                self.ridden_m += dist;
                if implied >= MOVING_MIN_MPS {
                    // Above the moving threshold: book distance *and* time toward Avg. Sub-threshold
                    // creep adds to `ridden_m` but not here, so distance and time stay paired.
                    self.moving_m += dist;
                    self.moving_s += dt;
                    // Sensor summaries share the moving-time weight (this interval's Δms) and accrue
                    // only while a *fresh* value is present — so a red-light stop (below the gate)
                    // and a dropped strap (stale) both stop the average cleanly. The pass drains the
                    // sensors before this, so `now_ms` gates against this pass's samples.
                    self.accumulate_sensors(now_ms, now_ms.saturating_sub(prev_ms));
                }
                counted = true;
            }
        }
        // Log the segment anchor (first fix) and every sane fix. A dropped fix (gap / teleport)
        // isn't logged and arms a segment break, so the drawn line and the GPX `<trkseg>` don't leap
        // across the hole.
        let log = first || counted;
        let segment_start = first || self.segment_break;
        self.segment_break = !log;
        self.last_fix = Some(fix);
        self.last_ms = Some(now_ms);
        Motion { log, segment_start }
    }

    /// Fold this moving interval's fresh sensor values into the per-ride summaries, weighted by
    /// `dt_ms` (the same Δms `moving_s` books). A stale value contributes nothing, so the average
    /// reflects only the time a sensor was actually reporting.
    fn accumulate_sensors(&mut self, now_ms: u32, dt_ms: u32) {
        if let Some(bpm) = self.live_hr(now_ms) {
            self.hr_ms_sum += bpm as u64 * dt_ms as u64;
            self.hr_ms += dt_ms;
            self.max_hr = self.max_hr.max(bpm);
        }
        if let Some(watts) = self.live_power(now_ms) {
            self.power_ms_sum += watts as u64 * dt_ms as u64;
            self.power_ms += dt_ms;
            self.max_power = self.max_power.max(watts);
        }
        if let Some(rpm) = self.live_cadence(now_ms) {
            self.cadence_ms_sum += rpm as u64 * dt_ms as u64;
            self.cadence_ms += dt_ms;
        }
    }

    /// Zero every accumulator, every integration anchor and the whole sensor summary — what a fresh
    /// ride starts from. The altimeter calibration is not one of them (see
    /// [`altitude`](Self::altitude)).
    pub(crate) fn reset_totals(&mut self) {
        self.ridden_m = 0.0;
        self.moving_m = 0.0;
        self.moving_s = 0.0;
        self.climb = DeadBand::new();
        self.last_fix = None;
        self.last_ms = None;
        self.last_alt = None;
        self.segment_break = false;
        // The live values self-heal through the staleness gate (a >5 s old sample already reads
        // `None`), so only the accumulators reset.
        self.hr_ms_sum = 0;
        self.hr_ms = 0;
        self.max_hr = 0;
        self.power_ms_sum = 0;
        self.power_ms = 0;
        self.max_power = 0;
        self.cadence_ms_sum = 0;
        self.cadence_ms = 0;
    }

    // ══ What the ride reads back ═══════════════════════════════════════════════════════════════

    /// Distance actually pedalled (m) — the `done` stat.
    pub fn ridden_m(&self) -> f32 {
        self.ridden_m
    }

    /// Moving time (s) — the `ride time` stat and the average's denominator.
    pub fn moving_s(&self) -> f32 {
        self.moving_s
    }

    /// Average speed (km/h) over the moving time, or `None` before any moving time has accrued (so
    /// the Statistics screen shows a placeholder, not `NaN`). Moving-only distance over moving time,
    /// so sub-threshold creep (counted in `ridden_m`) can't inflate it.
    pub fn avg_kmh(&self) -> Option<f32> {
        (self.moving_s > 0.0).then(|| self.moving_m / self.moving_s * 3.6)
    }

    /// Climb actually done (m) — barometric and dead-banded — the `climbed` stat.
    pub fn climb_m(&self) -> f32 {
        self.climb.ascent()
    }

    /// The **raw barometric** elevation (m): the latest altimeter sample, or `None` before the
    /// first. Unlike [`climb_m`](Self::climb_m) (dead-banded *ascent*) this is the present height
    /// and follows the altimeter in any mode. Absolute value is uncalibrated, so display goes
    /// through [`current_elevation_m`](Self::current_elevation_m) instead.
    pub fn baro_elevation_m(&self) -> Option<f32> {
        self.last_alt
    }

    /// The current elevation (m) **to show**: the map-referenced fused value once the estimator has
    /// settled (EL8), otherwise the raw barometric reading. `None` before the first altimeter
    /// sample. On a map with no terrain beside it the estimator never settles, so this is
    /// [`baro_elevation_m`](Self::baro_elevation_m) forever.
    pub fn current_elevation_m(&self) -> Option<f32> {
        let baro = self.last_alt?;
        Some(self.altitude.fused_m(baro).unwrap_or(baro))
    }

    /// The map-referenced altimeter's state (EL8) — the inspection surface the board's RTT line and
    /// the simulator's readout print; no UI reads it.
    pub fn altitude(&self) -> &AltitudeFusion {
        &self.altitude
    }

    /// Live heart rate (bpm) for the tile, or `None` when none has arrived or the last sample is
    /// older than [`SENSOR_STALE_MS`] — a dropped strap reads `--`, never its frozen last value.
    pub fn live_hr(&self, now_ms: u32) -> Option<u16> {
        self.hr_last.filter(|_| now_ms.saturating_sub(self.hr_at_ms) <= SENSOR_STALE_MS)
    }

    /// Live power (W) — the staleness twin of [`live_hr`](Self::live_hr).
    pub fn live_power(&self, now_ms: u32) -> Option<u16> {
        self.power_last.filter(|_| now_ms.saturating_sub(self.power_at_ms) <= SENSOR_STALE_MS)
    }

    /// Live cadence (rpm), or `None` when stale / never seen. A fresh `Some(0)` is a coasting rider
    /// (distinct from `None`), so the tile shows `0`, not `--`.
    pub fn live_cadence(&self, now_ms: u32) -> Option<u8> {
        self.cadence_last.filter(|_| now_ms.saturating_sub(self.cadence_at_ms) <= SENSOR_STALE_MS)
    }

    /// Live heart rate for a **stat tile**, judged against the last pass's ride clock rather than a
    /// render-time clock — see [`sensor_now_ms`](Self::sensor_now_ms).
    pub fn live_hr_display(&self) -> Option<u16> {
        self.live_hr(self.sensor_now_ms)
    }

    /// Live power for a stat tile — the display-clock twin of [`live_hr_display`](Self::live_hr_display).
    pub fn live_power_display(&self) -> Option<u16> {
        self.live_power(self.sensor_now_ms)
    }

    /// Live cadence for a stat tile — the display-clock twin of [`live_hr_display`](Self::live_hr_display).
    pub fn live_cadence_display(&self) -> Option<u8> {
        self.live_cadence(self.sensor_now_ms)
    }

    /// Average heart rate (bpm) over HR-present moving time, or `None` before any sample.
    pub fn avg_hr(&self) -> Option<u8> {
        (self.hr_ms > 0).then(|| (self.hr_ms_sum / self.hr_ms as u64).min(u8::MAX as u64) as u8)
    }

    /// Peak heart rate (bpm) seen during moving time, or `None` before any sample.
    pub fn max_hr(&self) -> Option<u8> {
        (self.hr_ms > 0).then(|| self.max_hr.min(u8::MAX as u16) as u8)
    }

    /// Average power (W) over power-present moving time, or `None` before any sample.
    pub fn avg_power(&self) -> Option<u16> {
        (self.power_ms > 0).then(|| (self.power_ms_sum / self.power_ms as u64).min(u16::MAX as u64) as u16)
    }

    /// Peak power (W) seen during moving time, or `None` before any sample.
    pub fn max_power(&self) -> Option<u16> {
        (self.power_ms > 0).then_some(self.max_power)
    }

    /// Average cadence (rpm) over cadence-present moving time — coasting-at-0 counts — or `None`
    /// before any sample.
    pub fn avg_cadence(&self) -> Option<u8> {
        (self.cadence_ms > 0).then(|| (self.cadence_ms_sum / self.cadence_ms as u64).min(u8::MAX as u64) as u8)
    }

    /// The ride's footer facts: the totals as they stand, against the anchor stamped when this
    /// operation was minted. The executor reads it as it writes the footer.
    pub fn ride_stats(&self) -> RideStats {
        RideStats {
            distance_m: self.ridden_m as u32, // float→int casts saturate
            moving_time_s: self.moving_s as u32,
            avg_speed_cms: if self.moving_s > 0.0 { (self.moving_m / self.moving_s * 100.0) as u16 } else { 0 },
            climb_m: self.climb.ascent() as u16,
            unix_at_anchor: self.clock.unix_at_anchor,
            anchor_ms: self.clock.anchor_ms,
            clock_trusted: self.clock.trusted,
            // Each is `None` (→ the codec's sentinel) when the ride saw no fresh sample of that
            // quantity (epic #707, SE3).
            avg_hr: self.avg_hr(),
            max_hr: self.max_hr(),
            avg_cadence: self.avg_cadence(),
            avg_power: self.avg_power(),
            max_power: self.max_power(),
        }
    }

    /// Snapshot every accumulator needed to continue footer totals exactly after a reset.
    pub fn continuation(&self) -> RideContinuation {
        RideContinuation {
            ridden_m: self.ridden_m,
            moving_m: self.moving_m,
            moving_s: self.moving_s,
            climb_m: self.climb.ascent(),
            descent_m: self.climb.descent(),
            hr_ms_sum: self.hr_ms_sum,
            hr_ms: self.hr_ms,
            max_hr: self.max_hr,
            power_ms_sum: self.power_ms_sum,
            power_ms: self.power_ms,
            max_power: self.max_power,
            cadence_ms_sum: self.cadence_ms_sum,
            cadence_ms: self.cadence_ms,
        }
    }

    /// Restore a recovered checkpoint's totals, before the rider's Continue opens the session that
    /// keeps them. The anchors stay dropped — a reboot is a sampling gap — so the first post-boot
    /// sample re-anchors and starts a fresh segment.
    pub fn restore_continuation(&mut self, state: RideContinuation) {
        self.ridden_m = state.ridden_m;
        self.moving_m = state.moving_m;
        self.moving_s = state.moving_s;
        self.climb = DeadBand::from_totals(state.climb_m, state.descent_m);
        self.last_fix = None;
        self.last_ms = None;
        self.segment_break = true;
        self.hr_ms_sum = state.hr_ms_sum;
        self.hr_ms = state.hr_ms;
        self.max_hr = state.max_hr;
        self.power_ms_sum = state.power_ms_sum;
        self.power_ms = state.power_ms;
        self.max_power = state.max_power;
        self.cadence_ms_sum = state.cadence_ms_sum;
        self.cadence_ms = state.cadence_ms;
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
            samples,
            clock,
            ridden_m: _,
            moving_m: _,
            moving_s: _,
            last_fix,
            last_ms,
            climb: _,
            last_alt,
            altitude: _,
            segment_break,
            hr_last,
            hr_at_ms: _,
            power_last,
            power_at_ms: _,
            cadence_last,
            cadence_at_ms: _,
            sensor_now_ms: _,
            hr_ms_sum: _,
            hr_ms: _,
            max_hr: _,
            power_ms_sum: _,
            power_ms: _,
            max_power: _,
            cadence_ms_sum: _,
            cadence_ms: _,
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
        assert!(samples.is_empty(), "no sample is waiting to be written");
        assert_eq!(*clock, FooterClock::default(), "no pass has stamped a footer anchor");
        assert!(last_fix.is_none() && last_ms.is_none() && last_alt.is_none(), "no fix and no altitude");
        assert!(!*segment_break, "no gap to break a segment across");
        assert!(hr_last.is_none() && power_last.is_none() && cadence_last.is_none(), "no strap has reported");
        self.assert_totals_are_zero();
    }

    /// Assert every accumulator reads its fresh-ride value — the whole rider-visible tally, so a
    /// summary a session edge forgot to clear is a failure here rather than a wrong number on glass.
    pub(crate) fn assert_totals_are_zero(&self) {
        assert_eq!(self.ridden_m, 0.0, "no distance");
        assert_eq!(self.moving_s, 0.0, "no moving time");
        assert_eq!(self.avg_kmh(), None, "no average");
        assert_eq!(self.climb_m(), 0.0, "no climb");
        assert_eq!(self.continuation(), RideContinuation::default(), "and nothing to continue");
        assert_eq!((self.avg_hr(), self.max_hr()), (None, None), "no heart-rate summary");
        assert_eq!((self.avg_power(), self.max_power()), (None, None), "no power summary");
        assert_eq!(self.avg_cadence(), None, "no cadence summary");
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

    /// A point near Berlin, and ~45 microdegrees of latitude ≈ 5.0 m north — roughly one second of
    /// riding at ~5 m/s, comfortably inside the [`MOVING_MIN_MPS`]..[`MAX_SPEED_MPS`] band.
    const LON: i32 = 13_405_000;
    const BASE_LAT: i32 = 52_520_000;
    const STEP_UD: i32 = 45;

    /// This pass's footer anchor, on a trusted clock — what stage 7 stamps as it offers the slot.
    fn at(ms: u32) -> FooterClock {
        FooterClock { unix_at_anchor: 1_720_000_000, anchor_ms: ms, trusted: true }
    }

    fn recording() -> RecorderMachine {
        let mut rec = RecorderMachine::new();
        rec.request(RecorderIntent::Start);
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Opened(SessionStart::Fresh));
        rec
    }

    /// A ride at one fix per second, `steps` fixes long, each ~5 m north of the last. Returns the
    /// machine with its samples staged and nothing yet written.
    fn ridden(steps: u32) -> RecorderMachine {
        let mut rec = recording();
        for step in 0..steps {
            assert!(!rec.record_fix(Fix::at(BASE_LAT + STEP_UD * step as i32, LON), step * 1000, true));
        }
        rec
    }

    /// Serve an [`Append`](RecorderEffect::Append) by writing `written` of the staged samples.
    fn append(rec: &mut RecorderMachine, now_ms: u32, written: u16) -> u16 {
        let effect = rec.next_effect(CAN_RECORD, at(now_ms)).expect("staged samples owe an append");
        let RecorderEffect::Append { token, samples } = effect else { panic!("expected an append: {effect:?}") };
        rec.apply_outcome(RecorderOutcome::Appended { token, samples: written });
        samples
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
        assert!(rec.next_effect(NO_STORE, at(60_000)).is_none(), "and nothing physical is offered either");
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

    /// A recovered ride the rider discarded cannot come back as the next ride's totals.
    ///
    /// The continuation edge is armed by the rider's Continue and spent by the session it opens.
    /// Anything else that ends the recovered ride — a close named instead, or the discard landing —
    /// must clear it, or `SessionStart::Recovered` would tell the pass to keep accumulators that
    /// belong to a ride the rider threw away.
    #[test]
    fn a_discarded_recovery_does_not_continue_into_the_next_ride() {
        let mut rec = RecorderMachine::new();
        assert!(rec.offer_recovery());
        rec.continue_recovered(); // armed…
        rec.request(RecorderIntent::Discard); // …and the rider changes their mind

        let effect = rec.next_effect(CAN_RECORD, at(1)).expect("the recovered object is discardable");
        rec.apply_outcome(RecorderOutcome::Discarded { token: effect.token() });

        rec.request(RecorderIntent::Start);
        assert_eq!(
            rec.advance(CAN_RECORD),
            RecorderAdvance::Opened(SessionStart::Fresh),
            "the next ride starts from zero, not from the ride that was thrown away"
        );
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
        let effect = rec.next_effect(CAN_RECORD, at(1)).expect("a finalize");
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
        let effect = rec.next_effect(CAN_RECORD, at(1)).expect("a discard");
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
        let effect = rec.next_effect(CAN_RECORD, at(1)).expect("the close outranks the cadence");
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
        let effect = rec.next_effect(CAN_RECORD, at(1)).expect("a finalize");
        let verdict = rec.apply_outcome(RecorderOutcome::Failed { token: effect.token(), error: RecorderError::Write });
        assert_eq!(verdict, RecorderVerdict::Failed);
        assert!(rec.recording(), "the ride the store still holds is still open");
        assert!(matches!(rec.next_effect(CAN_RECORD, at(2)), Some(RecorderEffect::Finalize { .. })), "and it retries");
    }

    /// One operation at a time, and a close offered into a pass that cannot carry it is kept, not
    /// destroyed.
    #[test]
    fn a_close_offered_while_the_slot_is_busy_is_not_lost() {
        let mut rec = recording();
        let checkpoint = rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS)).expect("the cadence came due");
        assert!(matches!(checkpoint, RecorderEffect::Checkpoint { .. }));

        rec.request(RecorderIntent::Save);
        assert!(rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS + 1)).is_none(), "one operation at a time");
        rec.apply_outcome(RecorderOutcome::Checkpointed { token: checkpoint.token() });
        assert!(
            matches!(rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS + 2)), Some(RecorderEffect::Finalize { .. })),
            "the rider's Save survived the busy pass"
        );
    }

    /// "Save & start new": the fresh ride opens only once the store has answered for the old one.
    #[test]
    fn save_and_restart_opens_the_new_ride_behind_the_close() {
        let mut rec = recording();
        let first = rec.session();
        rec.save_and_restart();
        let effect = rec.next_effect(CAN_RECORD, at(1)).expect("the close goes first");
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
        let stale = rec.next_effect(CAN_RECORD, at(1)).expect("a finalize");
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
        assert!(rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS - 1)).is_none(), "not yet due");
        let first = rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS)).expect("due");
        rec.apply_outcome(RecorderOutcome::Checkpointed { token: first.token() });
        assert!(rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS + 1)).is_none(), "the deadline moved with it");

        let second = rec.next_effect(CAN_RECORD, at(2 * CHECKPOINT_MS)).expect("due again");
        rec.apply_outcome(RecorderOutcome::Failed { token: second.token(), error: RecorderError::Write });
        assert!(
            matches!(rec.next_effect(CAN_RECORD, at(2 * CHECKPOINT_MS + 1)), Some(RecorderEffect::Checkpoint { .. })),
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
        let effect = rec.next_effect(CAN_RECORD, at(1)).expect("the recovered object is discardable");
        assert!(matches!(effect, RecorderEffect::Discard { .. }));
        assert_eq!(rec.apply_outcome(RecorderOutcome::Discarded { token: effect.token() }), RecorderVerdict::Dropped);
        assert!(rec.next_effect(CAN_RECORD, at(2)).is_none(), "and nothing is left to act on");
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
        let effect = rec.next_effect(CAN_RECORD, at(1)).expect("a discard");
        rec.apply_outcome(RecorderOutcome::Discarded { token: effect.token() });
        rec.request(RecorderIntent::Start);
        assert_eq!(
            rec.advance(CAN_RECORD),
            RecorderAdvance::Opened(SessionStart::Fresh),
            "the continuation edge was one-shot"
        );
    }

    // ══ Sample assembly ════════════════════════════════════════════════════════════════════════

    /// A ride stages one sample per logged fix and offers them as one append, and the executor's
    /// answer is what retires them.
    #[test]
    fn a_logged_fix_becomes_a_staged_sample_the_append_retires() {
        let mut rec = ridden(3);
        assert_eq!(rec.staged().len(), 3, "three logged fixes, three samples");
        assert!(rec.staged()[0].segment_start, "the first fix of a ride opens a segment");
        assert!(!rec.staged()[1].segment_start, "a continuous ride stays one segment");

        assert_eq!(append(&mut rec, 3_000, 3), 3, "the append names how many are ready");
        assert!(rec.staged().is_empty(), "and the answer retires exactly what reached the medium");
        assert!(rec.next_effect(CAN_RECORD, at(3_001)).is_none(), "nothing staged, nothing owed");
    }

    /// **A busy slot does not lose a sample.** One operation at a time, so a fix that lands while a
    /// checkpoint is with the executor stays staged and leaves with the next append.
    #[test]
    fn staged_samples_survive_a_busy_append_slot() {
        let mut rec = recording();
        let checkpoint = rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS)).expect("the cadence came due");
        assert!(matches!(checkpoint, RecorderEffect::Checkpoint { .. }));

        rec.record_fix(Fix::at(BASE_LAT, LON), CHECKPOINT_MS, true);
        rec.record_fix(Fix::at(BASE_LAT + STEP_UD, LON), CHECKPOINT_MS + 1_000, true);
        assert!(rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS + 1_000)).is_none(), "one operation at a time");
        assert_eq!(rec.staged().len(), 2, "and the busy pass destroyed neither of them");

        rec.apply_outcome(RecorderOutcome::Checkpointed { token: checkpoint.token() });
        assert_eq!(append(&mut rec, CHECKPOINT_MS + 1_001, 2), 2, "both leave with the next append");
        assert!(rec.staged().is_empty());
    }

    /// **A partial write is a delay, not a hole.** Only the prefix the medium took is retired; the
    /// rest is re-offered, in order, as the next append.
    #[test]
    fn a_partial_append_leaves_the_unwritten_samples_staged() {
        let mut rec = ridden(3);
        let third = rec.staged()[2];

        assert_eq!(append(&mut rec, 3_000, 1), 3, "three were offered");
        assert_eq!(rec.staged().len(), 2, "one reached the medium, two did not");

        assert_eq!(append(&mut rec, 3_001, 2), 2, "the retry offers exactly what is left");
        assert!(rec.staged().is_empty());
        // The order survived the partial: the last sample written is the last fix recorded.
        assert_eq!(third.t_ms, 2_000, "and the tail of the batch is the tail of the ride");
    }

    /// **A failed append advances nothing.** The medium took no sample, so none is retired and the
    /// ride's own totals — which the footer is read from — are exactly what the fixes made them.
    #[test]
    fn a_failed_append_does_not_advance_the_running_totals() {
        let mut rec = ridden(3);
        let before = rec.continuation();
        let staged = rec.staged().to_vec();

        let effect = rec.next_effect(CAN_RECORD, at(3_000)).expect("an append");
        let stats = rec.ride_stats();
        let verdict = rec.apply_outcome(RecorderOutcome::Failed { token: effect.token(), error: RecorderError::Write });
        assert_eq!(verdict, RecorderVerdict::Failed, "and the rider is told");

        assert_eq!(rec.staged(), staged.as_slice(), "every sample is still owed, in order");
        assert_eq!(rec.continuation(), before, "and a write that never happened credited nothing");
        assert_eq!(rec.ride_stats(), stats, "so the footer facts are exactly what the fixes made them");
        assert_eq!(append(&mut rec, 3_001, 3), 3, "the retry is the same batch");
    }

    /// **The checkpoint outranks the append**, and that is what stops a blocked journal starving its
    /// own repair: an executor that refuses samples until the exact failed write lands would never
    /// see that write if an append could hold the slot in front of it.
    #[test]
    fn a_checkpoint_that_is_owed_goes_before_the_staged_samples() {
        let mut rec = ridden(2);
        let checkpoint = rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS)).expect("the cadence came due");
        assert!(matches!(checkpoint, RecorderEffect::Checkpoint { .. }), "{checkpoint:?}");
        rec.apply_outcome(RecorderOutcome::Failed { token: checkpoint.token(), error: RecorderError::Write });

        let retry = rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS + 1)).expect("the blocked journal owes its repair");
        assert!(matches!(retry, RecorderEffect::Checkpoint { .. }), "the repair still outranks the samples: {retry:?}");
        rec.apply_outcome(RecorderOutcome::Checkpointed { token: retry.token() });
        assert_eq!(append(&mut rec, CHECKPOINT_MS + 2, 2), 2, "and the samples follow it");
    }

    /// The staging buffer is bounded. Past it the sample is gone, the rider is told, and the next
    /// sample that lands opens a fresh segment rather than drawing a line across the hole.
    #[test]
    fn a_full_staging_buffer_reports_the_lost_sample_and_breaks_the_segment() {
        let mut rec = ridden(STAGED_SAMPLES as u32);
        assert_eq!(rec.staged().len(), STAGED_SAMPLES);
        let overflow = STAGED_SAMPLES as u32;
        assert!(
            rec.record_fix(Fix::at(BASE_LAT + STEP_UD * overflow as i32, LON), overflow * 1000, true),
            "a sample that cannot be staged is a hole in the log, and the rider hears about it"
        );

        assert_eq!(append(&mut rec, 1, STAGED_SAMPLES as u16), STAGED_SAMPLES as u16);
        let next = overflow + 1;
        rec.record_fix(Fix::at(BASE_LAT + STEP_UD * next as i32, LON), next * 1000, true);
        assert!(rec.staged()[0].segment_start, "the fix after the hole starts a fresh segment");
    }

    /// Nothing is staged without a ride: the trail still grows so the Map can draw where the rider
    /// went, but there is no log to write into and no append to owe.
    #[test]
    fn a_fix_outside_a_ride_stages_nothing() {
        let mut rec = RecorderMachine::new();
        assert!(!rec.record_fix(Fix::at(BASE_LAT, LON), 0, true));
        assert!(rec.staged().is_empty(), "no session, no samples");
        assert!(!rec.breadcrumb.is_empty(), "but the trail is still the rider's");
    }

    /// A new session clears every accumulator — the totals, the integration anchors, the sensor
    /// summary and any sample the previous ride never wrote.
    #[test]
    fn a_new_session_clears_every_accumulator() {
        let mut rec = ridden(4);
        rec.record_hr(150, 4_000);
        rec.record_power(240, 4_000);
        rec.record_cadence(88, 4_000);
        rec.record_altitude(100.0, true);
        rec.record_fix(Fix::at(BASE_LAT + STEP_UD * 4, LON), 4_000, true);
        rec.record_altitude(130.0, true);
        assert!(rec.ridden_m() > 0.0 && rec.moving_s() > 0.0 && rec.climb_m() > 0.0, "ride one accumulated");
        assert_eq!(rec.avg_hr(), Some(150));
        assert!(!rec.staged().is_empty());

        // The session edge the pass applies for a fresh ride.
        rec.reset_totals();
        rec.restart_buffers();

        rec.assert_totals_are_zero();
        assert!(rec.staged().is_empty(), "and the old ride's unwritten samples go with it");
        assert!(rec.breadcrumb.is_empty(), "…as does its trail");
        assert_eq!(rec.baro_elevation_m(), None, "ride two re-anchors its own altitude");
    }

    // ══ The integration gates ══════════════════════════════════════════════════════════════════

    /// A real 1 Hz fix stream integrates distance and moving time and is **never** rejected as a
    /// teleport.
    #[test]
    fn one_hz_fix_stream_integrates_without_teleport_rejection() {
        let rec = ridden(5);
        assert!((16.0..=24.0).contains(&rec.ridden_m()), "four ~5 m steps ≈ 20 m, got {}", rec.ridden_m());
        assert_eq!(rec.moving_s(), 4.0, "every 1 s interval counts toward moving time");
        let avg = rec.avg_kmh().expect("moving time accrued");
        assert!((10.0..=25.0).contains(&avg), "~18 km/h, got {avg}");
        assert_eq!(rec.staged().len(), 5, "and every one of them is logged");
    }

    /// A stopped rider still emits fresh, identical-position fixes. They keep logging (the ride
    /// records the stop) but book no distance and no moving time, and are not a segment break.
    #[test]
    fn stationary_fixes_log_but_book_no_distance() {
        let mut rec = recording();
        for s in 0..=3u32 {
            rec.record_fix(Fix::at(BASE_LAT, LON), s * 1000, true);
        }
        assert_eq!(rec.staged().len(), 4, "an identical fix is a real sample, still logged");
        assert!(rec.staged()[1..].iter().all(|p| !p.segment_start), "standing still is not a segment break");
        assert_eq!(rec.ridden_m(), 0.0);
        assert_eq!(rec.avg_kmh(), None);
    }

    /// Avg must pair moving distance with moving time: a sub-threshold creep interval adds to the
    /// `done` total but *not* to Avg, so it cannot drag the average above any speed actually held.
    #[test]
    fn sub_threshold_creep_does_not_inflate_avg() {
        const CREEP_UD: i32 = 5; // ~0.56 m/s, below MOVING_MIN_MPS
        let mut rec = recording();
        rec.record_fix(Fix::at(BASE_LAT, LON), 0, true);
        rec.record_fix(Fix::at(BASE_LAT + STEP_UD, LON), 1000, true);
        rec.record_fix(Fix::at(BASE_LAT + STEP_UD + CREEP_UD, LON), 2000, true);

        assert_eq!(rec.moving_s(), 1.0, "only the above-threshold interval counts as moving time");
        assert!(rec.ridden_m() > 5.2, "creep distance is still in the done total, got {}", rec.ridden_m());
        let avg = rec.avg_kmh().expect("moving time accrued");
        let inflated = rec.ridden_m() / rec.moving_s() * 3.6;
        assert!((16.0..=19.5).contains(&avg), "avg must track the moving step, got {avg}");
        assert!(avg < inflated, "moving-only avg ({avg}) must be below the creep-inflated {inflated}");
    }

    /// **A dropped fix must not book a straight-line jump.** Both holes are skipped rather than
    /// integrated — a GPS dropout longer than the gate, and a teleport faster than a bicycle — and
    /// each opens a fresh track segment so nothing drawn or recorded leaps across it.
    #[test]
    fn a_dropped_fix_does_not_book_a_straight_line_jump() {
        // A 30 s reconnect only ~5 m away: inside the speed gate, past the gap gate.
        let mut gap = recording();
        gap.record_fix(Fix::at(BASE_LAT, LON), 0, true);
        gap.record_fix(Fix::at(BASE_LAT + STEP_UD, LON), 30_000, true);
        assert_eq!(gap.ridden_m(), 0.0, "the dropout interval is skipped, not booked");
        assert_eq!(gap.staged().len(), 1, "and the fix that ends it is not logged");
        gap.record_fix(Fix::at(BASE_LAT + 2 * STEP_UD, LON), 31_000, true);
        assert!(gap.staged()[1].segment_start, "resume starts a fresh segment");

        // ~111 m/s in one GPS period: inside the gap gate, past the speed gate.
        let mut jump = recording();
        jump.record_fix(Fix::at(BASE_LAT, LON), 0, true);
        jump.record_fix(Fix::at(BASE_LAT + 1_000, LON), 1000, true);
        assert_eq!(jump.ridden_m(), 0.0, "no impossible distance is ever credited");
        assert_eq!(jump.staged().len(), 1, "the teleport itself is not logged");
        jump.record_fix(Fix::at(BASE_LAT + 1_000 + STEP_UD, LON), 2000, true);
        assert!(jump.staged()[1].segment_start, "and the sane fix after it opens a segment");

        // The date line reads as a ~40 000 km jump through the same gate rather than crashing.
        let mut dateline = recording();
        dateline.record_fix(Fix::at(0, 179_999_990), 0, true);
        dateline.record_fix(Fix::at(0, -179_999_990), 1000, true);
        assert_eq!(dateline.ridden_m(), 0.0, "no planet-circling distance is ever booked");
    }

    /// Two fixes stamped the same millisecond are coalesced: the duplicate logs nothing and does
    /// **not** arm a segment break, so the next real fix integrates normally rather than being
    /// rejected as an infinite-speed teleport.
    #[test]
    fn same_millisecond_duplicate_is_coalesced_not_a_teleport() {
        let mut rec = recording();
        rec.record_fix(Fix::at(BASE_LAT, LON), 1000, true);
        rec.record_fix(Fix::at(BASE_LAT + STEP_UD, LON), 1000, true);
        assert_eq!(rec.staged().len(), 1, "a same-instant fix isn't logged");
        assert_eq!(rec.ridden_m(), 0.0, "no distance booked on a zero-length interval");

        rec.record_fix(Fix::at(BASE_LAT + 2 * STEP_UD, LON), 2000, true);
        assert!(!rec.staged()[1].segment_start, "the coalesced duplicate left no hole");
        assert!((4.0..=6.0).contains(&rec.ridden_m()), "one ~5 m step, got {}", rec.ridden_m());
        assert_eq!(rec.moving_s(), 1.0);
    }

    /// Outside the riding mode nothing is integrated and the anchor is dropped, so resuming can't
    /// book the distance covered while paused.
    #[test]
    fn a_paused_ride_drops_its_anchor_and_books_nothing() {
        let mut rec = recording();
        rec.record_fix(Fix::at(BASE_LAT, LON), 0, false);
        rec.record_fix(Fix::at(BASE_LAT + STEP_UD, LON), 1000, false);
        assert_eq!(rec.ridden_m(), 0.0);
        assert!(rec.staged().is_empty(), "a paused ride logs nothing");
    }

    /// The teleport gate is `implied < MAX_SPEED_MPS` and the moving gate `implied >=
    /// MOVING_MIN_MPS` — both boundaries pinned against an off-by-one.
    #[test]
    fn the_motion_gates_hold_at_their_exact_boundaries() {
        // dt = 100 ms: ~2.9 m is 29 m/s (counted), ~5 m is 50 m/s (dropped).
        let mut speed = recording();
        speed.record_fix(Fix::at(BASE_LAT, LON), 1000, true);
        speed.record_fix(Fix::at(BASE_LAT + 26, LON), 1100, true);
        assert!(speed.ridden_m() > 2.5 && speed.ridden_m() < 3.3, "29 m/s is under the gate: {}", speed.ridden_m());
        speed.record_fix(Fix::at(BASE_LAT + 26 + STEP_UD, LON), 1200, true);
        assert!(speed.ridden_m() < 3.3, "50 m/s is over it and books nothing extra");

        // dt = 1 s: ~0.89 m/s is at/above 0.8 (moving), ~0.67 m/s is below it (distance only).
        let mut at_gate = recording();
        at_gate.record_fix(Fix::at(BASE_LAT, LON), 0, true);
        at_gate.record_fix(Fix::at(BASE_LAT + 8, LON), 1000, true);
        assert_eq!(at_gate.moving_s(), 1.0, "≈0.89 m/s is at/above 0.8");

        let mut below = recording();
        below.record_fix(Fix::at(BASE_LAT, LON), 0, true);
        below.record_fix(Fix::at(BASE_LAT + 6, LON), 1000, true);
        assert_eq!(below.moving_s(), 0.0, "≈0.67 m/s is below it");
        assert!(below.ridden_m() > 0.0, "but the creep distance is still in the done total");
    }

    /// Longitude lines converge, so a microdegree of longitude is almost no ground distance at 85°N.
    /// A polar fix stream must not manufacture a teleport from the raw longitude delta.
    #[test]
    fn motion_near_the_pole_shrinks_longitude_distance() {
        let mut rec = recording();
        rec.record_fix(Fix::at(85_000_000, 0), 0, true);
        rec.record_fix(Fix::at(85_000_000, 100), 1000, true);
        assert!(rec.ridden_m() < 3.0, "heavily foreshortened at 85°N, got {}", rec.ridden_m());
        assert!(rec.ridden_m() > 0.0, "but still a real, non-zero step");
    }

    // ══ Climb and altitude ═════════════════════════════════════════════════════════════════════

    /// The dead-band is why climb isn't baro noise: a sub-3 m wiggle books nothing and does not
    /// re-anchor, descending never subtracts, and a garbage sample can neither inflate nor poison
    /// the total.
    #[test]
    fn climb_is_dead_banded_ascent_only_and_survives_garbage_samples() {
        let mut rec = recording();
        rec.record_altitude(100.0, true); // the reference; books nothing on its own
        rec.record_altitude(102.9, true); // inside the 3.0 m band, and does not re-anchor
        assert_eq!(rec.climb_m(), 0.0);
        rec.record_altitude(105.0, true); // 5 m above the *still-100* reference
        assert_eq!(rec.climb_m(), 5.0);

        rec.record_altitude(99.0, true); // descent is not climb
        rec.record_altitude(105.0, true); // …and the re-climb books again
        assert_eq!(rec.climb_m(), 11.0, "two clean gains, and the dip between them subtracts nothing");

        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            rec.record_altitude(bad, true);
        }
        assert_eq!(rec.climb_m(), 11.0, "garbage is ignored, and it did not re-anchor either");
        rec.record_altitude(110.0, true);
        assert_eq!(rec.climb_m(), 16.0, "a good sample measures from the last good reference");
    }

    /// Pausing drops the dead-band *reference* but keeps the total, so a height change during a rest
    /// is not booked on resume. The staged elevation follows the latest sample in any mode.
    #[test]
    fn a_pause_drops_the_climb_reference_but_still_stamps_the_elevation() {
        let mut rec = recording();
        rec.record_altitude(100.0, true);
        rec.record_altitude(110.0, true);
        assert_eq!(rec.climb_m(), 10.0);

        rec.record_altitude(160.0, false); // +50 m of drift during the stop
        rec.record_altitude(160.0, false);
        assert_eq!(rec.climb_m(), 10.0, "a height change during the pause must not accrue");
        assert_eq!(rec.baro_elevation_m(), Some(160.0), "…but the reading is still the current one");

        rec.record_altitude(160.0, true); // resume re-anchors at 160
        rec.record_altitude(165.0, true);
        assert_eq!(rec.climb_m(), 15.0, "only genuine post-resume climb adds, got {}", rec.climb_m());

        rec.record_fix(Fix::at(BASE_LAT, LON), 0, true);
        assert_eq!(rec.staged()[0].ele, 165, "and the staged sample carries it, rounded toward zero");
    }

    /// The map-referenced altimeter is a calibration of the atmosphere, not a tally of the ride: a
    /// new session must not throw it away and re-settle from scratch.
    #[test]
    fn a_new_session_keeps_the_altimeter_calibration() {
        let mut rec = recording();
        for _ in 0..crate::altitude::SETTLE_SAMPLES {
            rec.record_altitude(1062.0, true); // the barometer reads 62 m high
            rec.record_map_elevation(1000);
        }
        let offset = rec.altitude().offset_m().expect("settled during ride one");

        rec.reset_totals();
        assert!(rec.altitude().settled(), "the calibration survives a new session");
        assert_eq!(rec.altitude().offset_m(), Some(offset), "…unchanged");
        assert_eq!(rec.current_elevation_m(), None, "no altitude sample yet on ride two");
        rec.record_altitude(1062.0, true);
        assert_eq!(rec.current_elevation_m(), Some(1000.0), "fused from the retained offset at once");
        assert_eq!(rec.baro_elevation_m(), Some(1062.0), "and the raw reading is untouched");
    }

    // ══ The live sensors and the per-ride summary ══════════════════════════════════════════════

    /// A fresh sample reads live and, over moving intervals, folds into the average and the max —
    /// weighted over *moving* time, so a sub-threshold creep contributes no sensor weight either.
    #[test]
    fn a_fresh_strap_reads_live_and_accumulates_over_moving_time() {
        let mut rec = recording();
        rec.record_hr(100, 0);
        rec.record_fix(Fix::at(BASE_LAT, LON), 0, true); // the anchor books no time
        rec.record_hr(100, 1000);
        rec.record_fix(Fix::at(BASE_LAT + STEP_UD, LON), 1000, true);
        rec.record_hr(200, 2000);
        rec.record_fix(Fix::at(BASE_LAT + 2 * STEP_UD, LON), 2000, true);
        assert_eq!(rec.live_hr(2000), Some(200), "a just-stamped sample reads live");

        // A creep interval at a wild 40 bpm — below the moving gate, so it must not count.
        rec.record_hr(40, 3000);
        rec.record_fix(Fix::at(BASE_LAT + 2 * STEP_UD + 5, LON), 3000, true);
        assert_eq!(rec.avg_hr(), Some(150), "the mean of the two moving intervals, creep ignored");
        assert_eq!(rec.max_hr(), Some(200), "and the peak of the counted intervals");
    }

    /// **A stale strap records absent, not its last value.** Past the staleness horizon the live
    /// accessor blanks, the summary stops accruing, and — the half a frozen tile would hide — the
    /// sample staged for the log carries `None` rather than the value that stopped arriving.
    #[test]
    fn a_stale_strap_records_absent_not_its_last_value() {
        let mut rec = recording();
        rec.record_fix(Fix::at(BASE_LAT, LON), 0, true);
        rec.record_hr(150, 1_000);
        rec.record_power(200, 1_000);
        rec.record_cadence(90, 1_000);
        rec.record_fix(Fix::at(BASE_LAT + STEP_UD, LON), 1_000, true);
        assert_eq!(rec.staged()[1].hr, Some(150), "a fresh strap is stamped onto the sample");
        assert_eq!(rec.live_power(6_000), Some(200), "5 s old is exactly at the gate, still fresh");
        assert_eq!(rec.live_power(6_001), None, "one millisecond past it, the strap is stale");

        let summary = (rec.avg_hr(), rec.avg_power(), rec.avg_cadence());
        assert_eq!(summary, (Some(150), Some(200), Some(90)), "the one fresh interval booked all three");

        // A later, unambiguously-moving fix with every strap now 6 s stale.
        rec.record_fix(Fix::at(BASE_LAT + 6 * STEP_UD, LON), 7_000, true);
        let sample = rec.staged()[2];
        assert_eq!((sample.hr, sample.power, sample.cadence), (None, None, None), "absent, never frozen");
        assert_eq!((rec.avg_hr(), rec.avg_power(), rec.avg_cadence()), summary, "and nothing further accrued");
    }

    /// Coasting reads a fresh `Some(0)` cadence, which *does* count toward the average (feet still,
    /// sensor present) — distinct from a strap-absent `None`, which doesn't.
    #[test]
    fn cadence_zero_while_coasting_counts_into_the_average() {
        let mut rec = recording();
        rec.record_cadence(90, 0);
        rec.record_fix(Fix::at(BASE_LAT, LON), 0, true);
        rec.record_cadence(90, 1000);
        rec.record_fix(Fix::at(BASE_LAT + STEP_UD, LON), 1000, true);
        rec.record_cadence(0, 2000); // coasting: a real, fresh 0
        rec.record_fix(Fix::at(BASE_LAT + 2 * STEP_UD, LON), 2000, true);
        assert_eq!(rec.avg_cadence(), Some(45), "a fresh coasting 0 counts, averaging 90 and 0 to 45");
        assert_eq!(rec.staged()[2].cadence, Some(0), "and the log records the 0, not absence");
    }

    /// A recovered ride's raw integration state round-trips, so its averages continue rather than
    /// restart — numerators and denominators, not the rounded footer figures.
    #[test]
    fn a_recovered_continuation_restores_the_raw_summary_state() {
        let state = RideContinuation {
            ridden_m: 12_345.5,
            moving_m: 12_000.25,
            moving_s: 2_400.0,
            climb_m: 321.0,
            descent_m: 123.0,
            hr_ms_sum: 150 * 90_000,
            hr_ms: 90_000,
            max_hr: 188,
            power_ms_sum: 245 * 80_000,
            power_ms: 80_000,
            max_power: 901,
            cadence_ms_sum: 87 * 70_000,
            cadence_ms: 70_000,
        };
        let mut rec = RecorderMachine::new();
        rec.restore_continuation(state);
        assert_eq!(rec.continuation(), state);
        assert_eq!((rec.avg_hr(), rec.avg_power(), rec.avg_cadence()), (Some(150), Some(245), Some(87)));
        assert_eq!(rec.climb_m(), 321.0);
    }

    /// The footer facts are the ride's own totals against the anchor the pass stamped as it offered
    /// the slot — one assembly, in the machine that mints the close.
    #[test]
    fn the_footer_facts_come_from_recorder_at_finalize() {
        let mut rec = ridden(5);
        rec.record_hr(150, 4_000);
        rec.record_fix(Fix::at(BASE_LAT + 5 * STEP_UD, LON), 5_000, true);
        rec.record_altitude(100.0, true);
        rec.record_altitude(140.0, true);

        rec.request(RecorderIntent::Save);
        let clock = FooterClock { unix_at_anchor: 1_720_000_500, anchor_ms: 5_000, trusted: true };
        let effect = rec.next_effect(CAN_RECORD, clock).expect("the close outranks the staged samples");
        assert!(matches!(effect, RecorderEffect::Finalize { .. }), "{effect:?}");

        let stats = rec.ride_stats();
        assert_eq!(stats.distance_m, rec.ridden_m() as u32, "the ride's own distance");
        assert_eq!(stats.moving_time_s, rec.moving_s() as u32);
        assert_eq!(stats.climb_m, 40, "and its own climb");
        assert_eq!(stats.avg_hr, Some(150), "and its own sensor summary");
        assert_eq!(
            (stats.unix_at_anchor, stats.anchor_ms, stats.clock_trusted),
            (1_720_000_500, 5_000, true),
            "against the anchor the pass stamped as this close was minted"
        );
    }
}
