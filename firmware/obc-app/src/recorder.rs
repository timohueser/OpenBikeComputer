//! The Recorder domain: the ride session and its persistence lifecycle.
//!
//! [`RecorderMachine`] owns the ride identity, when a ride starts, when a checkpoint is owed, and
//! what "closed" means. An executor writes bytes and reports what happened. A close stays open
//! until the executor answers it, so a failed finalize keeps the ride the store still holds.
//!
//! Sample batches never ride an effect: Recorder stages them in its own bounded buffer and an
//! [`Append`](RecorderEffect::Append) names how many are ready. Distance, moving time, climb and
//! the per-ride sensor summary accrue here, so a session edge is the only thing that can zero
//! them.

pub mod continuation;

use obc_elevation::DeadBand;
use obc_formats::bike::BikeType;
use obc_formats::ride::{Name, TripRef};
use obc_map_scene::ground_dist_m;
use obc_ports::{Fix, TrackPoint};
use obc_route::RideStats;

use crate::altitude::AltitudeFusion;
use crate::breadcrumb::Breadcrumb;
use crate::device_core::{OperationToken, RecorderCapabilities, RecorderTag, TokenSource};
use crate::effort::{Effort, EffortLimits, Gauge, Metric, Reading};
use crate::placement::define_placement_constructors;

use crate::CatalogObjectId;

/// How long a ride may go unjournalled. The cadence is the domain's, so every executor gives the
/// same ride the same recovery window.
const CHECKPOINT_MS: u32 = 10_000;

/// Wait for the board storage transport's recovery window before a failed checkpoint is offered
/// again. The board asserts that this equals its breaker's cool-down.
pub const CHECKPOINT_RETRY_MS: u32 = 30_000;

/// How many assembled samples Recorder may hold before an executor has written them. Sized against
/// the executor's delta window (the board's `DELTA_SAMPLES`), so the domain never stages more than
/// one checkpoint interval's worth.
const STAGED_SAMPLES: usize = 16;

/// A gap longer than this between fixes (s) is a GPS dropout, not travel. The interval is skipped,
/// so a reconnect does not book a straight-line jump across it.
const MAX_GAP_S: f32 = 10.0;
/// Implied speed above this (m/s) is a glitch, not riding. The interval is skipped rather than
/// crediting impossible distance.
const MAX_SPEED_MPS: f32 = 30.0;
/// Below this implied speed (m/s) the rider is stopped. The time does not count toward the moving
/// average, so rests do not drag Avg. Speed down.
const MOVING_MIN_MPS: f32 = 0.8;
/// A BLE sensor sample older than this (ms) is stale: the live accessors read `None` and the summary
/// stops accumulating it. A dropped strap records absent, never its last value.
const SENSOR_STALE_MS: u32 = 5_000;
/// The longest interval one power sample is credited for. A meter reports every second or faster,
/// so a longer gap is a dropout, and a dropout adds no energy.
const ENERGY_GAP_MS: u32 = 2_000;

/// The wall-clock anchor a ride footer carries. The pass gives it to Recorder as it offers the
/// operation slot, which pairs the anchor with the samples that operation writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FooterClock {
    /// UTC seconds at [`anchor_ms`](Self::anchor_ms), or 0 with no trusted clock.
    pub unix_at_anchor: u32,
    /// The map-plane instant [`unix_at_anchor`](Self::unix_at_anchor) was read at.
    pub anchor_ms: u32,
    /// Whether the wall clock has a trusted source (GPS or BLE).
    pub trusted: bool,
}

/// What one fix did to the ride log: whether it was logged (fed the trail and staged a sample) and
/// whether it starts a new track segment — the first fix of a session, or the first after a pause or
/// a GPS gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Motion {
    log: bool,
    segment_start: bool,
}

/// What a ride records about its start: the current bike type, the trip day it started on and the
/// rider's effort limits. A later settings change does not reach a ride already started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RideOrigin {
    pub bike: BikeType,
    pub trip: Option<TripRef>,
    pub limits: EffortLimits,
}

/// The state that must cross a reset when a journaled ride is continued.
///
/// It is the raw integration state, not the rounded footer summary: averages need their numerators
/// and denominators to merge post-reset samples without drift. Position and elevation anchors are
/// absent, so the first post-boot fix re-anchors instead of booking movement across power-off
/// time.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RideContinuation {
    pub origin: RideOrigin,
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
    /// The ride's energy from power (J), or `None` before any power sample.
    pub energy_j: Option<u32>,
}

/// Whether a completed checkpoint service established recovery on its medium.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointStatus {
    Durable,
    /// The medium has no durable recovery guarantee, so no durable claim is made.
    Unsupported,
}

/// What a store did when an executor asked it to close the open ride. Only `Failed` is a retry: a
/// store that could not tell "there was nothing to close" from "the close failed" would put the
/// domain in a retry loop against an object that does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RideClose {
    /// The store committed the ride under this identity.
    Committed(CatalogObjectId),
    /// There was no open ride, so the close saved nothing.
    Nothing,
    /// The close did not happen and the ride is still there. Recorder re-offers the same one.
    Failed,
}

/// What the rider asks of the ride recorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderIntent {
    Start,
    /// Close the open ride and keep it as a durable ride object.
    Save,
    /// Close the open ride and throw it away.
    Discard,
}

/// One bounded physical recording operation, carrying the [`OperationToken`] Recorder issued.
///
/// There is no `Start` effect: starting is a state change, and the first physical work a ride
/// causes is its first [`Append`](RecorderEffect::Append).
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
    /// The medium refused or failed the write. Recorder keeps the samples staged and retries.
    Write,
    /// The store will take no further mutation this boot — an exhausted revision or sequence space,
    /// not a write that went wrong. Retrying cannot help, which is why it is not a
    /// [`Write`](RecorderError::Write).
    ReadOnly,
}

/// What is wrong with a durable `RECORDING` object no session could be attached to.
///
/// `Payload` and `Metadata` have a repair: the exact removal of that one entry. `Catalog` has none
/// this domain may attempt — a catalog it could not read completely is not one it may mutate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RideDamage {
    /// The recovered bytes are not a ride sample/footer boundary.
    Payload,
    /// The recovered samples carry no decodable continuation image.
    Metadata,
    /// The catalog could not be listed completely, or does not hold the recovered key.
    Catalog,
}

/// What the boot-recovered object is, and how far the rider has got with deciding about it.
///
/// The terminal answers differ in one thing that matters: which effects this machine may still
/// mint. [`Unrepairable`](RideRecoveryState::Unrepairable) may mint none, ever.
///
/// The removal is one operation with two subjects, so the attempt and its latch carry what the
/// object was: `Some(damage)` is a damaged recording being repaired, `None` a whole recovered ride
/// the rider chose to throw away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum RideRecoveryState {
    /// Nothing was recovered, or the decision is over.
    #[default]
    None,
    /// A whole recovered ride, waiting on Continue or Discard.
    Resumable,
    /// Logical damage on a catalog this executor read completely. One rider-confirmed removal
    /// repairs it.
    Repairable(RideDamage),
    /// The rider's one confirmed removal is with the executor now.
    Attempting(Option<RideDamage>),
    /// That attempt failed and nothing further happens automatically this boot. A fresh rider
    /// confirmation buys exactly one more.
    Latched(Option<RideDamage>),
    /// There is no safe object-level operation left: an unreadable catalog, or a store that will
    /// take no mutation at all. The card names it and this machine writes nothing.
    Unrepairable,
}

impl RideRecoveryState {
    /// The state a boot-classified damage is offered in. Damage to the catalog is the one cause
    /// with no repair to offer.
    pub(crate) fn for_damage(damage: RideDamage) -> Self {
        match damage {
            RideDamage::Catalog => RideRecoveryState::Unrepairable,
            RideDamage::Payload | RideDamage::Metadata => RideRecoveryState::Repairable(damage),
        }
    }

    /// Whether an object the executor holds may still be acted on. `Unrepairable` answers `false`,
    /// so the owner, not the executor, is what keeps an unreadable catalog untouched.
    fn holds_object(self) -> bool {
        matches!(
            self,
            RideRecoveryState::Resumable
                | RideRecoveryState::Repairable(_)
                | RideRecoveryState::Attempting(_)
                | RideRecoveryState::Latched(_)
        )
    }

    /// Whether an undecided object is still standing between the rider and recording. A session
    /// opened against one records nothing, because that object still owns the store's ride state.
    pub(crate) fn blocks_recording(self) -> bool {
        !matches!(self, RideRecoveryState::None | RideRecoveryState::Resumable)
    }

    /// The rider's confirmation becomes the one attempt, whichever object it is about.
    fn attempting(self) -> Self {
        match self {
            RideRecoveryState::Resumable => RideRecoveryState::Attempting(None),
            RideRecoveryState::Repairable(damage) => RideRecoveryState::Attempting(Some(damage)),
            RideRecoveryState::Latched(damage) => RideRecoveryState::Attempting(damage),
            other => other,
        }
    }
}

/// The result of one [`RecorderEffect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderOutcome {
    /// `samples` staged points reached the medium.
    Appended {
        token: OperationToken<RecorderTag>,
        samples: u16,
    },
    /// Checkpoint service finished; only `Durable` establishes recovery.
    Checkpointed {
        token: OperationToken<RecorderTag>,
        status: CheckpointStatus,
    },
    /// No samples were accepted; checkpoint the previous boundary before retrying the batch.
    NeedsCheckpoint {
        token: OperationToken<RecorderTag>,
    },
    /// The ride is closed and committed under `ride`.
    Finalized {
        token: OperationToken<RecorderTag>,
        ride: CatalogObjectId,
    },
    /// The open ride is gone.
    Discarded {
        token: OperationToken<RecorderTag>,
    },
    Failed {
        token: OperationToken<RecorderTag>,
        error: RecorderError,
    },
    /// The executor abandoned the operation without completing it.
    Cancelled {
        token: OperationToken<RecorderTag>,
    },
}

impl RecorderOutcome {
    pub fn token(&self) -> OperationToken<RecorderTag> {
        match self {
            RecorderOutcome::Appended { token, .. }
            | RecorderOutcome::Checkpointed { token, .. }
            | RecorderOutcome::NeedsCheckpoint { token }
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
    /// The device cannot record, so no session opened, and the request was kept. Reported once, on
    /// the pass that first refuses it: a rider who believes a ride is recording and it is not would
    /// ride it, end it, and find nothing saved.
    Refused,
    /// A damaged recovered object is still standing, so no session opened. The request is kept and
    /// the recovery decision is put back to the rider, once per ask.
    RecoveryOwed,
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
    /// The rider's one repair attempt on a damaged recovered object failed, and Recorder has latched
    /// the terminal state for it. Nothing more is attempted automatically; the card names the
    /// latch, which is how the rider tries again.
    RecoveryLatched,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckpointOwed {
    No,
    Immediate,
    Backoff,
}

/// The one owner of the ride lifecycle: the session identity and its monotonic source, whether a
/// ride is open, the rider's undelivered request, the checkpoint deadline, the boot-recovery
/// decision, and the per-session buffers a new ride restarts.
///
/// It produces exactly one thing: a bounded [`RecorderEffect`], offered while
/// [`RecorderCapabilities::record`] holds. Nothing else decides that a ride is open or closed.
pub struct RecorderMachine {
    /// The open ride's session id, or `None` when no ride is open. A ride the rider has closed is
    /// still open here until the executor's verdict lands — a finalize that failed closed nothing.
    session: Option<u32>,
    /// Monotonic id source for [`session`](Self::session), so a new session is never mistaken for
    /// the one it replaced.
    seq: u32,
    /// The map-plane instant the last checkpoint was issued at. The deadline is
    /// [`CHECKPOINT_MS`] past it.
    last_checkpoint_ms: u32,
    /// The token source for the one recording operation that may be in flight.
    ops: TokenSource<RecorderTag>,
    /// The operation currently with the executor, so a failure re-arms the right thing.
    inflight: Option<InFlight>,
    /// The rider's undelivered request. A [`Start`](RecorderIntent::Start) is consumed by the pass
    /// that opens the session; a close is kept until the executor confirms it, so a failed or
    /// refused close re-offers rather than evaporating.
    pending: Option<RecorderIntent>,
    /// The next session continues a recovered journal, so its restored totals must survive the
    /// start. Armed by [`continue_recovered`](Self::continue_recovered) and spent by the open.
    resume_next: bool,
    /// "Save & start new" is two rider decisions in one gesture: the open ride closes and a fresh
    /// one opens behind it. The new ride may not open until the store has answered for the old one,
    /// or the two would share a session.
    restart_after_close: bool,
    /// The rider has been told that this device cannot record. One card per ask, not one per pass:
    /// the request stays pending, so without this the warning would re-raise for ever.
    /// [`request`](Self::request) clears it, so asking again is answered again.
    refusal_told: bool,
    /// A blocked journal keeps its checkpoint owed until the exact same write lands. A media
    /// failure backs off until the storage recovery probe; local refusals retry at once.
    checkpoint_owed: CheckpointOwed,
    /// Whether a boot-recovered ride has already been put to the rider. A recorder may report the
    /// same resumable object every pass; the rider sees one decision card.
    recovery_offered: bool,
    /// What the executor recovered and where the rider's decision about it has got to. It belongs
    /// to no session, which is what lets a `Discard` become an effect with no session open.
    ///
    /// It also bounds automatic writes: each rider confirmation buys exactly one removal attempt,
    /// and `Unrepairable` admits none at all.
    recovery: RideRecoveryState,
    /// The travelled-path breadcrumb (RAM, bounded), fed each logged fix and drawn on the Map.
    /// Per-session: a new ride starts with an empty trail.
    pub(crate) breadcrumb: Breadcrumb,

    /// The assembled samples no executor has written yet, and the one sample queue in the device.
    /// An [`Append`](RecorderEffect::Append) names how many are ready and the answer says how many
    /// reached the medium; the rest stay here.
    samples: heapless::Vec<TrackPoint, STAGED_SAMPLES>,
    /// The footer's wall-clock anchor, as of the pass that last offered an operation slot.
    clock: FooterClock,

    /// Distance actually pedalled (m) — the `done` stat. Counts every sane fix, including
    /// sub-threshold creep, so it is the true total covered.
    ridden_m: f32,
    /// Distance covered while moving (m): only fixes at or above [`MOVING_MIN_MPS`], and the
    /// numerator of Avg. Kept separate from [`ridden_m`](Self::ridden_m) so the average pairs
    /// moving distance with moving time.
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
    /// The map-referenced altimeter: the slow estimate of the barometer's absolute offset, fed one
    /// terrain sample per GPS fix. It corrects what the Elevation tile shows and nothing else —
    /// [`last_alt`](Self::last_alt), and so the recorded track and the climb dead-band, stays raw
    /// barometry (see [`crate::altitude`]). [`reset_totals`](Self::reset_totals) leaves it alone: it
    /// calibrates the atmosphere, not the ride.
    altitude: AltitudeFusion,
    /// `true` when a dropped fix (GPS gap / teleport) left a hole, so the next staged sample starts
    /// a fresh track segment.
    segment_break: bool,

    // Live BLE sensor values. Each holds the last sample and the ride-clock ms it arrived; the
    // `live_*` accessors return `None` once it is older than SENSOR_STALE_MS.
    hr_last: Option<u16>,
    hr_at_ms: u32,
    power_last: Option<u16>,
    power_at_ms: u32,
    cadence_last: Option<u8>,
    cadence_at_ms: u32,
    /// The ride-clock ms of the most recent pass — the timebase samples record on. The
    /// `live_*_display` accessors judge staleness against it, so a stat tile rendered after the pass
    /// compares like-for-like with the record clock instead of spuriously blanking.
    sensor_now_ms: u32,

    // Per-ride sensor summary, time-weighted over moving time and accruing only while a fresh
    // value is present. The weight is the interval's Δms (`_ms`), the sum is value×Δms (`_ms_sum`),
    // and the quotient is the moving-time average.
    hr_ms_sum: u64,
    hr_ms: u32,
    max_hr: u16,
    power_ms_sum: u64,
    power_ms: u32,
    max_power: u16,
    /// Σ(rpm × Δms) over cadence-present moving time, and its Δms denominator. A fresh `0` while
    /// coasting counts; a strap that is absent does not.
    cadence_ms_sum: u64,
    cadence_ms: u32,
    /// The ride's energy (J): each power sample times the time since the one before it, at most
    /// [`ENERGY_GAP_MS`], while the ride records. `None` until the first sample.
    energy_j: Option<u32>,
    /// The live effort display: zones, smoothed power and the graph history. Not per ride, so no
    /// session edge resets it.
    effort: Effort,
    /// Set when a fresh ride opens. A continued ride restores it with the totals.
    origin: RideOrigin,
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
        /// Initialize `slot` in place to the [`new`](RecorderMachine::new) state. The breadcrumb is
        /// KB-scale, so nothing here may form a by-value `RecorderMachine` on the stack.
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
            checkpoint_owed: CheckpointOwed::No,
            recovery_offered: false,
            recovery: RideRecoveryState::None,
            breadcrumb: Breadcrumb::new() => Breadcrumb::init_in_place,

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
            energy_j: None,
            effort: Effort::new(),
            origin: RideOrigin { bike: BikeType::Road, trip: None, limits: EffortLimits { max_hr: 0, ftp_w: 0 } },
        }
    );

    /// Name what the rider wants of the recorder. A screen calls it as the gesture happens, through
    /// `Ctx::recorder`.
    ///
    /// A second request before the first is acted on replaces it. Replacing the
    /// [`Save`](RecorderIntent::Save) a "save & start new" armed disarms the restart with it.
    pub fn request(&mut self, intent: RecorderIntent) {
        if intent != RecorderIntent::Save {
            self.restart_after_close = false;
        }
        if intent != RecorderIntent::Start {
            // Leaving the continuation armed would carry a discarded ride's restored totals into
            // whatever ride the rider starts next.
            self.resume_next = false;
        }
        // A fresh ask gets a fresh answer, so a rider who pressed START again is told again.
        self.refusal_told = false;
        self.pending = Some(intent);
    }

    /// Close the open ride, then open a fresh one the moment the store confirms the close.
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

    /// The open ride's session id — the level an executor keys its ride log on.
    pub fn session(&self) -> Option<u32> {
        self.session
    }

    /// Whether a ride is open (recording, paused, or closing).
    pub fn recording(&self) -> bool {
        self.session.is_some()
    }

    /// A requested or issued close still owns this session until its terminal outcome lands.
    pub fn closing(&self) -> bool {
        matches!(self.pending, Some(RecorderIntent::Save | RecorderIntent::Discard))
            || matches!(self.inflight, Some(InFlight::Close))
    }

    /// Put a boot-recovered ride to the rider in `state`, once per boot. `false` means the decision
    /// was already offered or a ride is already open; recovery can never replace a live session.
    pub(crate) fn offer_recovery(&mut self, state: RideRecoveryState) -> bool {
        if self.recovery_offered || self.session.is_some() {
            return false;
        }
        self.recovery_offered = true;
        self.recovery = state;
        true
    }

    pub(crate) fn recovery(&self) -> RideRecoveryState {
        self.recovery
    }

    /// Advance the domain one pass: turn the rider's [`Start`](RecorderIntent::Start) into a session.
    ///
    /// Starting is a state change, not an effect, so it happens here rather than in
    /// [`next_effect`](Self::next_effect). It is gated on the capability: a ride with nowhere to put
    /// it is not a ride.
    ///
    /// A refused start is kept, not thrown away, so a device whose card mounts a pass later opens
    /// the ride the rider asked for. What the rider must not get is silence — see
    /// [`Refused`](RecorderAdvance::Refused).
    pub(crate) fn advance(&mut self, caps: RecorderCapabilities) -> RecorderAdvance {
        if !matches!(self.pending, Some(RecorderIntent::Start)) {
            return RecorderAdvance::Nothing;
        }
        if !caps.record {
            // Kept, and `resume_next` with it: a pass that opened nothing must not spend the
            // recovered ride's continuation edge.
            if core::mem::replace(&mut self.refusal_told, true) {
                return RecorderAdvance::Nothing;
            }
            return RecorderAdvance::Refused;
        }
        // An undecided object still owns the store's ride state, so a session opened against it
        // would accept fixes, refuse every write, and save nothing — silently. The request is kept,
        // like a refused one, and the decision goes back to the rider instead.
        if self.recovery.blocks_recording() {
            if core::mem::replace(&mut self.refusal_told, true) {
                return RecorderAdvance::Nothing;
            }
            return RecorderAdvance::RecoveryOwed;
        }
        self.pending = None;
        self.refusal_told = false;
        let resume = core::mem::take(&mut self.resume_next);
        self.seq = self.seq.wrapping_add(1);
        self.session = Some(self.seq);
        self.checkpoint_owed = CheckpointOwed::No;
        // Adopted by this session, or never there to begin with.
        self.recovery = RideRecoveryState::None;
        RecorderAdvance::Opened(if resume { SessionStart::Recovered } else { SessionStart::Fresh })
    }

    /// The ride object an executor owes: the open session's id, unless `opened` already names it.
    ///
    /// The id, not a boolean: an executor that served a close has not yet run the pass that applies
    /// the verdict, so it still sees an open session here. A boolean could not tell that apart from
    /// "the start failed, retry", and would open a second object under the closing ride's identity.
    ///
    /// `opened` is what the executor has already opened an object for. A close never clears it.
    pub fn object_owed(&self, opened: Option<u32>) -> Option<u32> {
        match self.session {
            Some(id) if opened != Some(id) => Some(id),
            _ => None,
        }
    }

    /// The one bounded operation this pass may carry, or `None`. `clock` is the footer's wall-clock
    /// anchor as of this pass, stamped here so the figures an executor writes belong to the
    /// operation it is about to perform.
    ///
    /// Everything physical is refused without a writable store. One operation at a time, in a fixed
    /// rank:
    ///
    /// 1. Discard, because explicit removal needs no drain or journal repair.
    /// 2. The checkpoint, because an executor whose journal is blocked refuses samples until the
    ///    exact failed write lands — an append that outranked it would starve its own repair.
    /// 3. The append, which retires only the samples the executor acknowledges.
    /// 4. Finalize, once a pending Save has no staged samples or checkpoint repair left.
    ///
    /// A close is not consumed here. It stays [`pending`](Self::pending) until the executor's
    /// verdict retires it.
    pub(crate) fn next_effect(&mut self, caps: RecorderCapabilities, clock: FooterClock) -> Option<RecorderEffect> {
        self.clock = clock;
        // The anchor is this pass's map-plane instant, so the cadence reads its deadline from it.
        let now_ms = clock.anchor_ms;
        if !caps.record || self.inflight.is_some() {
            return None;
        }
        if self.session.is_none() && !self.recovery.holds_object() {
            return None;
        }
        match self.pending {
            Some(RecorderIntent::Save) if self.samples.is_empty() && self.checkpoint_owed == CheckpointOwed::No => {
                self.inflight = Some(InFlight::Close);
                return Some(RecorderEffect::Finalize { token: self.ops.issue() });
            }
            Some(RecorderIntent::Discard) => {
                // The rider's confirmation becomes the attempt. A removal that then fails latches
                // its terminal state instead of re-offering.
                self.recovery = self.recovery.attempting();
                self.inflight = Some(InFlight::Close);
                return Some(RecorderEffect::Discard { token: self.ops.issue() });
            }
            // `Start` is spent by `advance`; a stale one here would open nothing.
            Some(RecorderIntent::Save | RecorderIntent::Start) | None => {}
        }
        // Everything below belongs to a session: the cadence keeps a ride recoverable and the
        // append writes its samples. A recovered object has neither.
        self.session?;
        if self.checkpoint_due(now_ms) {
            self.last_checkpoint_ms = now_ms;
            self.checkpoint_owed = CheckpointOwed::No;
            self.inflight = Some(InFlight::Checkpoint);
            return Some(RecorderEffect::Checkpoint { token: self.ops.issue() });
        }
        if self.checkpoint_owed != CheckpointOwed::No {
            return None;
        }
        if self.samples.is_empty() {
            return None;
        }
        self.inflight = Some(InFlight::Append);
        Some(RecorderEffect::Append { token: self.ops.issue(), samples: self.samples.len() as u16 })
    }

    /// The samples an executor serving an [`Append`](RecorderEffect::Append) must write, in order.
    /// It writes a prefix and says how long it was.
    pub fn staged(&self) -> &[TrackPoint] {
        &self.samples
    }

    /// The context for a full issued batch, while the executor holds the App fixed.
    /// A later staged suffix must be reissued before it can share the current totals.
    pub fn append_context(&self, samples: u16) -> Option<RideContinuation> {
        (usize::from(samples) == self.samples.len()).then(|| self.continuation())
    }

    /// Only an empty staging queue permits a fresh context without accepting more samples.
    /// Otherwise the executor checkpoints its previous accepted boundary.
    pub fn checkpoint_context(&self) -> Option<RideContinuation> {
        self.samples.is_empty().then(|| self.continuation())
    }

    /// Consume the answer to a [`RecorderEffect`] and say what it means to the app.
    ///
    /// A stale token changes nothing, which stops a late answer closing a session that has since
    /// been replaced.
    #[cfg(test)]
    pub(crate) fn apply_outcome(&mut self, outcome: RecorderOutcome) -> RecorderVerdict {
        self.apply_outcome_at(outcome, self.clock.anchor_ms)
    }

    /// Apply an executor answer at the time DeviceCore received it. A failed checkpoint starts its
    /// retry window here, after the failed media operation and the transport breaker's verdict.
    pub(crate) fn apply_outcome_at(&mut self, outcome: RecorderOutcome, now_ms: u32) -> RecorderVerdict {
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
                // A failed journal write keeps its staged append. A failed checkpoint waits for
                // the storage recovery probe; an append failure still owes its repair at once.
                if error == RecorderError::Write {
                    match was {
                        Some(InFlight::Checkpoint) => {
                            self.checkpoint_owed = CheckpointOwed::Backoff;
                            self.last_checkpoint_ms = now_ms;
                        }
                        Some(InFlight::Append) => {
                            self.checkpoint_owed = CheckpointOwed::Immediate;
                        }
                        _ => {}
                    }
                }
                // The rider's one attempt is over: a failed removal latches what the store answered
                // and retires the request, so nothing further is minted without a fresh
                // confirmation. A store that refuses every mutation ends the decision outright, for
                // either object — it would not take a continued ride's writes either.
                if let RideRecoveryState::Attempting(damage) = self.recovery {
                    self.recovery = match error {
                        RecorderError::ReadOnly => RideRecoveryState::Unrepairable,
                        RecorderError::Write => RideRecoveryState::Latched(damage),
                    };
                    self.pending = None;
                    return RecorderVerdict::RecoveryLatched;
                }
                // A close stays pending, so it re-offers: the ride is still on the store.
                RecorderVerdict::Failed
            }
            RecorderOutcome::Cancelled { .. } => {
                if was == Some(InFlight::Checkpoint) {
                    self.checkpoint_owed = CheckpointOwed::Immediate;
                }
                // Nothing was attempted, so nothing is latched, but the confirmation went with the
                // abandoned operation.
                if let RideRecoveryState::Attempting(damage) = self.recovery {
                    self.recovery = match damage {
                        Some(damage) => RideRecoveryState::Repairable(damage),
                        None => RideRecoveryState::Resumable,
                    };
                    self.pending = None;
                }
                RecorderVerdict::Nothing
            }
            // Only the prefix the medium took is retired; the rest stays staged, so a partial write
            // is a delay rather than a hole in the ride log.
            RecorderOutcome::Appended { samples, .. } => {
                let taken = (samples as usize).min(self.samples.len());
                let keep = self.samples.len() - taken;
                if keep != 0 {
                    // A full executor delta needs a checkpoint before it can accept the tail.
                    self.checkpoint_owed = CheckpointOwed::Immediate;
                }
                self.samples.as_mut_slice().copy_within(taken.., 0);
                self.samples.truncate(keep);
                RecorderVerdict::Nothing
            }
            RecorderOutcome::NeedsCheckpoint { .. } => {
                self.checkpoint_owed = CheckpointOwed::Immediate;
                RecorderVerdict::Nothing
            }
            RecorderOutcome::Checkpointed { .. } => RecorderVerdict::Nothing,
        }
    }

    /// Clear the previous ride's trail and unwritten samples before starting a new ride.
    pub(crate) fn restart_buffers(&mut self) {
        self.breadcrumb.clear();
        self.samples.clear();
    }

    /// Whether a journal checkpoint may leave now. Local refusals retry at once; a media failure
    /// waits for the transport recovery probe, and an ordinary checkpoint follows the cadence.
    fn checkpoint_due(&self, now_ms: u32) -> bool {
        match self.checkpoint_owed {
            CheckpointOwed::No => now_ms.wrapping_sub(self.last_checkpoint_ms) >= CHECKPOINT_MS,
            CheckpointOwed::Immediate => true,
            CheckpointOwed::Backoff => now_ms.wrapping_sub(self.last_checkpoint_ms) >= CHECKPOINT_RETRY_MS,
        }
    }

    /// The ride is over: drop the identity and any owed cadence.
    fn close(&mut self) {
        self.session = None;
        self.samples.clear();
        self.checkpoint_owed = CheckpointOwed::No;
        // A committed removal is the repair: the object is gone, so the decision is over and the
        // next Start opens a fresh ride in this same boot.
        self.recovery = RideRecoveryState::None;
        // The "save & start new" second half, if the rider asked for one.
        self.pending = core::mem::take(&mut self.restart_after_close).then_some(RecorderIntent::Start);
    }

    /// Current pass clock used to attach a recovered physical recording.
    pub fn now_ms(&self) -> u32 {
        self.sensor_now_ms
    }

    /// Record the ride-clock ms of the current pass, so the `live_*_display` accessors judge
    /// freshness on the same clock samples record on.
    pub(crate) fn note_sensor_clock(&mut self, now_ms: u32) {
        self.sensor_now_ms = now_ms;
    }

    pub(crate) fn record_hr(&mut self, bpm: u16, now_ms: u32) {
        self.hr_last = Some(bpm);
        self.hr_at_ms = now_ms;
        self.effort.sample(Metric::Hr, bpm, now_ms);
    }

    /// Take one power sample. `riding` is false while paused or idle, and then no energy accrues.
    pub(crate) fn record_power(&mut self, watts: u16, now_ms: u32, riding: bool) {
        if riding && self.session.is_some() && self.pending != Some(RecorderIntent::Save) {
            let dt = self.power_last.map_or(0, |_| now_ms.saturating_sub(self.power_at_ms).min(ENERGY_GAP_MS));
            let joules = (watts as u32 * dt + 500) / 1000;
            self.energy_j = Some(self.energy_j.unwrap_or(0).saturating_add(joules));
        }
        self.power_last = Some(watts);
        self.power_at_ms = now_ms;
        self.effort.sample(Metric::Power, watts, now_ms);
    }

    /// Scroll the effort history to this pass and zone the latest values against the limits.
    pub(crate) fn advance_effort(&mut self, limits: EffortLimits) {
        let power = self.power_last.map(|_| self.effort.power());
        self.effort.advance(self.sensor_now_ms, limits, self.hr_last, power);
    }

    pub(crate) fn record_cadence(&mut self, rpm: u8, now_ms: u32) {
        self.cadence_last = Some(rpm);
        self.cadence_at_ms = now_ms;
    }

    /// Integrate one barometric altitude sample into the climbed total, dead-banded so sensor noise
    /// does not inflate it. `riding` is false while paused or idle: the reference is dropped so an
    /// altitude change during the pause is not booked on resume.
    pub(crate) fn record_altitude(&mut self, alt_m: f32, riding: bool) {
        // A non-finite sample would book infinite ascent and poison `climbed` permanently.
        if !alt_m.is_finite() {
            return;
        }
        // The latest altitude stamps staged samples in any mode; the dead-band only runs while
        // riding.
        self.last_alt = Some(alt_m);
        if !riding || self.pending == Some(RecorderIntent::Save) {
            self.climb.pause();
            return;
        }
        self.climb.push(alt_m);
    }

    /// Feed one terrain sample taken at the current GPS fix into the map-referenced altimeter. It
    /// pairs with the barometric reading from the same pass, so the residual is a like-for-like
    /// difference. A no-op before the first altimeter sample.
    pub(crate) fn record_map_elevation(&mut self, map_m: i16) {
        if let Some(baro) = self.last_alt {
            self.altitude.observe(f32::from(map_m), baro);
        }
    }

    /// Integrate one position fix into the ride, and stage the sample it produces.
    ///
    /// By the [`LocationSource`](obc_ports::LocationSource) contract this is called once per fresh
    /// GPS sample, so consecutive calls are a GPS period apart — the interval the gates are sized
    /// for. `riding` is false while paused or idle: nothing accumulates and nothing is logged.
    ///
    /// Returns `true` when the staging buffer was full and the log lost a sample, which raises the
    /// recording warning.
    pub(crate) fn record_fix(&mut self, fix: Fix, now_ms: u32, riding: bool) -> bool {
        let motion = self.integrate(fix, now_ms, riding && self.pending != Some(RecorderIntent::Save));
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
            // A strap that is dropped or stale records absent, never its frozen last value.
            hr: self.live_hr(now_ms).map(|b| b.min(u8::MAX as u16) as u8),
            cadence: self.live_cadence(now_ms),
            power: self.live_power(now_ms),
        };
        if self.samples.push(point).is_err() {
            // The buffer is a full checkpoint window deep and nothing drained it. The sample is
            // gone, so the next one starts a fresh segment rather than drawing a line across it.
            self.segment_break = true;
            return true;
        }
        false
    }

    /// The distance and time half of [`record_fix`](Self::record_fix).
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
            // A non-advancing clock cannot be integrated: `dist / dt` would manufacture an infinite
            // implied speed and reject the next real move as a teleport. The fix is coalesced into
            // the anchor instead, and it arms no segment break because no time elapsed.
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
                    // Above the moving threshold, distance and time both count toward Avg.
                    // Sub-threshold creep adds to `ridden_m` but not here, so the two stay paired.
                    self.moving_m += dist;
                    self.moving_s += dt;
                    // Sensor summaries share the moving-time weight and accrue only while a fresh
                    // value is present, so a stop and a dropped strap both stop the average
                    // cleanly.
                    self.accumulate_sensors(now_ms, now_ms.saturating_sub(prev_ms));
                }
                counted = true;
            }
        }
        // A dropped fix is not logged and arms a segment break, so the drawn line and the GPX
        // `<trkseg>` do not leap across the hole.
        let log = first || counted;
        let segment_start = first || self.segment_break;
        self.segment_break = !log;
        self.last_fix = Some(fix);
        self.last_ms = Some(now_ms);
        Motion { log, segment_start }
    }

    /// Fold this moving interval's fresh sensor values into the per-ride summaries, weighted by
    /// `dt_ms`. A stale value contributes nothing, so the average reflects only the time a sensor
    /// was reporting.
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
    /// ride starts from. The altimeter calibration is not one of them.
    pub(crate) fn reset_totals(&mut self) {
        self.ridden_m = 0.0;
        self.moving_m = 0.0;
        self.moving_s = 0.0;
        self.climb = DeadBand::new();
        self.last_fix = None;
        self.last_ms = None;
        self.last_alt = None;
        self.segment_break = false;
        // The live values self-heal through the staleness gate, so only the accumulators reset.
        self.hr_ms_sum = 0;
        self.hr_ms = 0;
        self.max_hr = 0;
        self.power_ms_sum = 0;
        self.power_ms = 0;
        self.max_power = 0;
        self.cadence_ms_sum = 0;
        self.cadence_ms = 0;
        self.energy_j = None;
    }

    /// Distance actually pedalled (m) — the `done` stat.
    pub fn ridden_m(&self) -> f32 {
        self.ridden_m
    }

    /// Moving time (s) — the `ride time` stat and the average's denominator.
    pub fn moving_s(&self) -> f32 {
        self.moving_s
    }

    /// Average speed (km/h) over the moving time, or `None` before any moving time has accrued.
    /// Moving-only distance over moving time, so sub-threshold creep cannot inflate it.
    pub fn avg_kmh(&self) -> Option<f32> {
        (self.moving_s > 0.0).then(|| self.moving_m / self.moving_s * 3.6)
    }

    /// Climb actually done (m) — barometric and dead-banded — the `climbed` stat.
    pub fn climb_m(&self) -> f32 {
        self.climb.ascent()
    }

    /// The raw barometric elevation (m): the latest altimeter sample, or `None` before the first.
    /// The absolute value is uncalibrated, so display goes through
    /// [`current_elevation_m`](Self::current_elevation_m) instead.
    pub fn baro_elevation_m(&self) -> Option<f32> {
        self.last_alt
    }

    /// The current elevation (m) to show: the map-referenced fused value once the estimator has
    /// settled, otherwise the raw barometric reading. On a map with no terrain the estimator never
    /// settles, so this is [`baro_elevation_m`](Self::baro_elevation_m) forever.
    pub fn current_elevation_m(&self) -> Option<f32> {
        let baro = self.last_alt?;
        Some(self.altitude.fused_m(baro).unwrap_or(baro))
    }

    /// The map-referenced elevation (m), or `None` until the estimator has settled. It never falls
    /// back to the uncalibrated barometric reading, so a consumer can treat it as an absolute
    /// height.
    pub fn fused_elevation_m(&self) -> Option<f32> {
        self.altitude.fused_m(self.last_alt?)
    }

    /// The map-referenced altimeter's state — an inspection surface for the board's RTT line and
    /// the simulator's readout. No UI reads it.
    pub fn altitude(&self) -> &AltitudeFusion {
        &self.altitude
    }

    /// Live heart rate (bpm) for the tile, or `None` when none has arrived or the last sample is
    /// older than [`SENSOR_STALE_MS`].
    pub fn live_hr(&self, now_ms: u32) -> Option<u16> {
        self.hr_last.filter(|_| now_ms.saturating_sub(self.hr_at_ms) <= SENSOR_STALE_MS)
    }

    /// Live power (W) — the staleness twin of [`live_hr`](Self::live_hr).
    pub fn live_power(&self, now_ms: u32) -> Option<u16> {
        self.power_last.filter(|_| now_ms.saturating_sub(self.power_at_ms) <= SENSOR_STALE_MS)
    }

    /// Live cadence (rpm), or `None` when stale or never seen. A fresh `Some(0)` is a coasting
    /// rider, so the tile shows `0`, not `--`.
    pub fn live_cadence(&self, now_ms: u32) -> Option<u8> {
        self.cadence_last.filter(|_| now_ms.saturating_sub(self.cadence_at_ms) <= SENSOR_STALE_MS)
    }

    /// Live heart rate for a stat tile, judged against the last pass's ride clock rather than a
    /// render-time clock.
    pub fn live_hr_display(&self) -> Option<u16> {
        self.live_hr(self.sensor_now_ms)
    }

    /// Live power for a stat tile — the twin of [`live_hr_display`](Self::live_hr_display).
    pub fn live_power_display(&self) -> Option<u16> {
        self.live_power(self.sensor_now_ms)
    }

    /// Live cadence for a stat tile — the twin of [`live_hr_display`](Self::live_hr_display).
    pub fn live_cadence_display(&self) -> Option<u8> {
        self.live_cadence(self.sensor_now_ms)
    }

    /// Live heart rate or 10 s power with its zone, for the tiles, the gauge and the graphs. `None`
    /// while the sensor is stale, judged like [`live_hr_display`](Self::live_hr_display).
    pub fn reading(&self, m: Metric) -> Option<Reading> {
        let value = match m {
            Metric::Hr => self.live_hr_display(),
            Metric::Power => self.live_power_display().map(|_| self.effort.power()),
        };
        value.map(|value| Reading { value, zone: self.effort.zone(m) })
    }

    /// The map's effort gauge for a `w` px wide panel, or `None` when no band shows.
    pub fn gauge(&self, limits: EffortLimits, w: i32) -> Option<Gauge> {
        crate::effort::gauge(self.reading(Metric::Power), self.reading(Metric::Hr), limits, w)
    }

    pub fn effort(&self) -> &Effort {
        &self.effort
    }

    /// The effort history's open bucket while `m`'s graph can still move: it holds a bar, or its
    /// sensor is live. `None` for a graph that stays empty, so it forces no repaint.
    pub fn graph_bucket(&self, m: Metric) -> Option<u32> {
        (self.reading(m).is_some() || self.effort.has_history(m)).then(|| self.effort.bucket())
    }

    /// The ride's energy (kJ), or `None` before any power sample.
    pub fn kj(&self) -> Option<u32> {
        self.energy_j.map(|j| j / 1000)
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

    pub(crate) fn set_origin(&mut self, origin: RideOrigin) {
        self.origin = origin;
    }

    /// The ride's footer facts: the totals as they stand, against the anchor stamped when this
    /// operation was minted. The trip name is App's to fill ([`App::ride_stats`](crate::App::ride_stats)).
    pub(crate) fn ride_stats(&self) -> RideStats {
        RideStats {
            distance_m: self.ridden_m as u32, // float→int casts saturate
            moving_time_s: self.moving_s as u32,
            avg_speed_cms: if self.moving_s > 0.0 { (self.moving_m / self.moving_s * 100.0) as u16 } else { 0 },
            climb_m: self.climb.ascent() as u16,
            descent_m: self.climb.descent() as u16,
            unix_at_anchor: self.clock.unix_at_anchor,
            anchor_ms: self.clock.anchor_ms,
            clock_trusted: self.clock.trusted,
            // `None` becomes the codec's sentinel when the ride saw no fresh sample.
            avg_hr: self.avg_hr(),
            max_hr: self.max_hr(),
            avg_cadence: self.avg_cadence(),
            avg_power: self.avg_power(),
            max_power: self.max_power(),
            energy_kj: self.kj(),
            bike: self.origin.bike,
            limits: self.origin.limits,
            trip: self.origin.trip,
            trip_name: Name::EMPTY,
        }
    }

    /// Snapshot every accumulator needed to continue footer totals exactly after a reset.
    pub fn continuation(&self) -> RideContinuation {
        RideContinuation {
            origin: self.origin,
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
            energy_j: self.energy_j,
        }
    }

    /// Restore a recovered checkpoint's totals, before the rider's Continue opens the session that
    /// keeps them. The anchors stay dropped, so the first post-boot sample re-anchors and starts a
    /// fresh segment.
    pub fn restore_continuation(&mut self, state: RideContinuation) {
        self.origin = state.origin;
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
        self.energy_j = state.energy_j;
    }
}

#[cfg(test)]
impl RecorderMachine {
    /// Open a session directly — the screen suites' stand-in for a mounted store.
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

    /// Assert the boot state field by field. The destructure is exhaustive, so a new field must
    /// state its boot value here too.
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
            recovery,
            breadcrumb,
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
            energy_j: _,
            effort: _,
            origin,
        } = self;
        assert!(session.is_none() && *seq == 0, "no ride has ever been open");
        assert_eq!(*last_checkpoint_ms, 0, "no checkpoint has been issued");
        assert!(inflight.is_none() && pending.is_none(), "nothing requested, nothing in flight");
        assert!(!*resume_next && !*restart_after_close, "no continuation and no restart armed");
        assert!(!*refusal_told, "the rider has not been refused a ride");
        assert_eq!(*checkpoint_owed, CheckpointOwed::No, "no checkpoint owed");
        assert!(!*recovery_offered && *recovery == RideRecoveryState::None, "no recovered ride offered this boot");
        assert!(breadcrumb.is_empty(), "no trail");

        assert!(samples.is_empty(), "no sample is waiting to be written");
        assert_eq!(*clock, FooterClock::default(), "no pass has stamped a footer anchor");
        assert!(last_fix.is_none() && last_ms.is_none() && last_alt.is_none(), "no fix and no altitude");
        assert!(!*segment_break, "no gap to break a segment across");
        assert!(hr_last.is_none() && power_last.is_none() && cadence_last.is_none(), "no strap has reported");
        assert_eq!(*origin, RideOrigin::default(), "no ride has started");
        self.assert_totals_are_zero();
    }

    /// Assert every accumulator reads its fresh-ride value, so a summary a session edge forgot to
    /// clear fails here rather than showing a wrong number on glass.
    pub(crate) fn assert_totals_are_zero(&self) {
        assert_eq!(self.ridden_m, 0.0, "no distance");
        assert_eq!(self.moving_s, 0.0, "no moving time");
        assert_eq!(self.avg_kmh(), None, "no average");
        assert_eq!(self.climb_m(), 0.0, "no climb");
        let zero = RideContinuation { origin: self.origin, ..RideContinuation::default() };
        assert_eq!(self.continuation(), zero, "and nothing to continue");
        assert_eq!((self.avg_hr(), self.max_hr()), (None, None), "no heart-rate summary");
        assert_eq!((self.avg_power(), self.max_power()), (None, None), "no power summary");
        assert_eq!(self.avg_cadence(), None, "no cadence summary");
        // The integration anchors, which no accessor above reads: a fresh ride that kept them
        // would credit itself with the step from the previous ride's last fix.
        assert!(self.last_fix.is_none() && self.last_ms.is_none(), "no anchor to integrate against");
        assert!(!self.segment_break, "and no hole for the next sample to break a segment across");
    }
}

// A recorder message is a token, a count, or a ride identity — never a batch.
const _: () = assert!(core::mem::size_of::<RecorderIntent>() <= 1, "three fieldless requests");
const _: () = assert!(core::mem::size_of::<RecorderEffect>() <= 8, "a token and a sample count");
const _: () = assert!(core::mem::size_of::<RecorderOutcome>() <= 16, "a token and a ride identity");
const _: () = assert!(core::mem::size_of::<RecorderError>() <= 1, "a verdict, not a report");

#[cfg(test)]
mod tests {
    use super::*;

    const CAN_RECORD: RecorderCapabilities = RecorderCapabilities { record: true };
    const NO_STORE: RecorderCapabilities = RecorderCapabilities { record: false };

    /// A point near Berlin. 45 microdegrees of latitude is about 5 m north, so one step per second
    /// sits inside the [`MOVING_MIN_MPS`]..[`MAX_SPEED_MPS`] band.
    const LON: i32 = 13_405_000;
    const BASE_LAT: i32 = 52_520_000;
    const STEP_UD: i32 = 45;

    fn at(ms: u32) -> FooterClock {
        FooterClock { unix_at_anchor: 1_720_000_000, anchor_ms: ms, trusted: true }
    }

    fn recording() -> RecorderMachine {
        let mut rec = RecorderMachine::new();
        rec.request(RecorderIntent::Start);
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Opened(SessionStart::Fresh));
        rec
    }

    /// A ride at one fix per second, `steps` fixes long, each ~5 m north of the last. Its samples
    /// are staged and nothing is written.
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

    #[test]
    fn recording_is_refused_without_a_writable_store() {
        let mut rec = RecorderMachine::new();
        rec.request(RecorderIntent::Start);
        assert_eq!(rec.advance(NO_STORE), RecorderAdvance::Refused, "no store, no ride — and say so");
        assert!(!rec.recording());
        assert!(rec.next_effect(NO_STORE, at(60_000)).is_none(), "and nothing physical is offered either");
        assert_eq!(rec.advance(NO_STORE), RecorderAdvance::Nothing, "one card per ask, not one per pass");

        rec.request(RecorderIntent::Start);
        assert_eq!(rec.advance(NO_STORE), RecorderAdvance::Refused, "a fresh ask gets a fresh answer");

        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Opened(SessionStart::Fresh), "the kept request opens it");
        assert!(rec.recording());
    }

    #[test]
    fn a_refused_continue_keeps_the_continuation_edge() {
        let mut rec = RecorderMachine::new();
        assert!(rec.offer_recovery(RideRecoveryState::Resumable));
        rec.continue_recovered();
        assert_eq!(rec.advance(NO_STORE), RecorderAdvance::Refused);
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Opened(SessionStart::Recovered), "still a continuation");
    }

    /// The continuation edge is armed by the rider's Continue and spent by the session it opens.
    /// Anything else that ends the recovered ride must clear it.
    #[test]
    fn a_discarded_recovery_does_not_continue_into_the_next_ride() {
        let mut rec = RecorderMachine::new();
        assert!(rec.offer_recovery(RideRecoveryState::Resumable));
        rec.continue_recovered();
        rec.request(RecorderIntent::Discard);

        let effect = rec.next_effect(CAN_RECORD, at(1)).expect("the recovered object is discardable");
        rec.apply_outcome(RecorderOutcome::Discarded { token: effect.token() });

        rec.request(RecorderIntent::Start);
        assert_eq!(
            rec.advance(CAN_RECORD),
            RecorderAdvance::Opened(SessionStart::Fresh),
            "the next ride starts from zero, not from the ride that was thrown away"
        );
    }

    /// The board serves the effect at the top of an iteration and runs the pass at the end of it,
    /// so `session()` is still `Some(N)` while the object is already gone.
    #[test]
    fn a_close_answered_before_its_verdict_owes_no_second_object() {
        let mut rec = recording();
        let first = rec.session().expect("a ride is open");
        assert_eq!(rec.object_owed(None), Some(first), "an unopened session owes its object");
        let opened = Some(first);
        assert_eq!(rec.object_owed(opened), None, "and an opened one owes nothing");

        rec.request(RecorderIntent::Save);
        let effect = rec.next_effect(CAN_RECORD, at(1)).expect("a finalize");
        assert_eq!(rec.object_owed(opened), None, "the closing ride must not be opened a second time");
        rec.apply_outcome(RecorderOutcome::Finalized { token: effect.token(), ride: 9 });
        assert_eq!(rec.object_owed(opened), None, "and neither must the closed one");

        rec.request(RecorderIntent::Start);
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Opened(SessionStart::Fresh));
        assert_eq!(rec.object_owed(opened), rec.session(), "a new ride owes a new object");
    }

    #[test]
    fn a_discard_after_an_armed_restart_opens_nothing_behind_it() {
        let mut rec = recording();
        rec.save_and_restart();
        rec.request(RecorderIntent::Discard);
        let effect = rec.next_effect(CAN_RECORD, at(1)).expect("a discard");
        assert!(matches!(effect, RecorderEffect::Discard { .. }));
        rec.apply_outcome(RecorderOutcome::Discarded { token: effect.token() });
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Nothing, "no ride follows a discard");
        assert!(!rec.recording());
    }

    #[test]
    fn the_close_becomes_a_finalize_and_the_session_survives_until_it_is_answered() {
        let mut rec = recording();
        assert!(!rec.closing());
        rec.request(RecorderIntent::Save);
        assert!(rec.closing());
        let effect = rec.next_effect(CAN_RECORD, at(1)).expect("the close outranks the cadence");
        assert!(matches!(effect, RecorderEffect::Finalize { .. }));
        assert!(rec.recording(), "the ride is open until the store says otherwise");

        let verdict = rec.apply_outcome(RecorderOutcome::Finalized { token: effect.token(), ride: 42 });
        assert_eq!(verdict, RecorderVerdict::Saved(42));
        assert!(!rec.recording());
        assert!(!rec.closing());
    }

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

    #[test]
    fn a_close_offered_while_the_slot_is_busy_is_not_lost() {
        let mut rec = recording();
        let checkpoint = rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS)).expect("the cadence came due");
        assert!(matches!(checkpoint, RecorderEffect::Checkpoint { .. }));

        rec.request(RecorderIntent::Save);
        assert!(rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS + 1)).is_none(), "one operation at a time");
        rec.apply_outcome(RecorderOutcome::Checkpointed {
            token: checkpoint.token(),
            status: CheckpointStatus::Durable,
        });
        assert!(
            matches!(rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS + 2)), Some(RecorderEffect::Finalize { .. })),
            "the rider's Save survived the busy pass"
        );
    }

    #[test]
    fn a_storage_recovery_pass_preserves_pending_save_and_samples() {
        let mut rec = ridden(3);
        let staged = rec.staged().to_vec();
        rec.request(RecorderIntent::Save);

        assert!(rec.next_effect(NO_STORE, at(3_000)).is_none(), "the recovery pass issues no store effect");
        assert_eq!(rec.staged(), staged, "accepted samples remain pending");
        assert!(
            matches!(rec.next_effect(CAN_RECORD, at(3_001)), Some(RecorderEffect::Append { samples: 3, .. })),
            "the next normal pass resumes Save in its existing order"
        );
    }

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

    #[test]
    fn a_failed_checkpoint_retries_once_at_the_recovery_deadline() {
        let mut rec = recording();
        assert!(rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS - 1)).is_none(), "not yet due");
        let first = rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS)).expect("due");
        rec.apply_outcome(RecorderOutcome::Checkpointed { token: first.token(), status: CheckpointStatus::Durable });
        assert!(rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS + 1)).is_none(), "the deadline moved with it");

        let second = rec.next_effect(CAN_RECORD, at(2 * CHECKPOINT_MS)).expect("due again");
        let failed_at = 2 * CHECKPOINT_MS + 8_000;
        rec.apply_outcome_at(RecorderOutcome::Failed { token: second.token(), error: RecorderError::Write }, failed_at);
        assert!(rec.next_effect(CAN_RECORD, at(failed_at)).is_none(), "no same-pass retry");
        assert!(
            rec.next_effect(CAN_RECORD, at(failed_at + CHECKPOINT_RETRY_MS - 1)).is_none(),
            "the failed media operation anchors the complete recovery window"
        );
        let retry = rec
            .next_effect(CAN_RECORD, at(failed_at + CHECKPOINT_RETRY_MS))
            .expect("one retry leaves exactly at the deadline");
        assert!(matches!(retry, RecorderEffect::Checkpoint { .. }));

        let failed_again_at = failed_at + CHECKPOINT_RETRY_MS + 8_000;
        rec.apply_outcome_at(
            RecorderOutcome::Failed { token: retry.token(), error: RecorderError::Write },
            failed_again_at,
        );
        assert!(rec.next_effect(CAN_RECORD, at(failed_again_at + CHECKPOINT_RETRY_MS - 1)).is_none());
        assert!(matches!(
            rec.next_effect(CAN_RECORD, at(failed_again_at + CHECKPOINT_RETRY_MS)),
            Some(RecorderEffect::Checkpoint { .. })
        ));
    }

    #[test]
    fn checkpoint_retry_deadline_survives_the_clock_wrap() {
        let mut rec = recording();
        let issued_at = u32::MAX - CHECKPOINT_MS;
        let checkpoint = rec.next_effect(CAN_RECORD, at(issued_at)).expect("the wrapped cadence is due");
        let failed_at = u32::MAX - CHECKPOINT_RETRY_MS / 2;
        rec.apply_outcome_at(
            RecorderOutcome::Failed { token: checkpoint.token(), error: RecorderError::Write },
            failed_at,
        );
        let retry_at = failed_at.wrapping_add(CHECKPOINT_RETRY_MS);
        assert!(retry_at < failed_at, "the test crosses the clock wrap");
        assert!(rec.next_effect(CAN_RECORD, at(retry_at.wrapping_sub(1))).is_none());
        assert!(matches!(rec.next_effect(CAN_RECORD, at(retry_at)), Some(RecorderEffect::Checkpoint { .. })));
    }

    #[test]
    fn a_recovered_ride_can_be_discarded_without_a_session() {
        let mut rec = RecorderMachine::new();
        assert!(rec.offer_recovery(RideRecoveryState::Resumable));
        rec.request(RecorderIntent::Discard);
        let effect = rec.next_effect(CAN_RECORD, at(1)).expect("the recovered object is discardable");
        assert!(matches!(effect, RecorderEffect::Discard { .. }));
        assert_eq!(rec.apply_outcome(RecorderOutcome::Discarded { token: effect.token() }), RecorderVerdict::Dropped);
        assert!(rec.next_effect(CAN_RECORD, at(2)).is_none(), "and nothing is left to act on");
    }

    #[test]
    fn a_recovered_ride_continues_without_resetting_its_totals() {
        let mut rec = RecorderMachine::new();
        assert!(rec.offer_recovery(RideRecoveryState::Resumable), "the decision is put to the rider");
        assert!(!rec.offer_recovery(RideRecoveryState::Resumable), "once per boot");

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

    /// Offer a recovered object in `offered` and take the one effect the rider's confirmation buys.
    /// Used for both subjects of the removal, because the operation is the same one.
    fn confirmed_removal(offered: RideRecoveryState) -> (RecorderMachine, RecorderEffect) {
        let mut rec = RecorderMachine::new();
        assert!(rec.offer_recovery(offered));
        rec.request(RecorderIntent::Discard);
        let effect = rec.next_effect(CAN_RECORD, at(1)).expect("the rider's confirmation buys one attempt");
        assert!(matches!(effect, RecorderEffect::Discard { .. }), "the removal is the exact one: {effect:?}");
        (rec, effect)
    }

    fn repairing(damage: RideDamage) -> (RecorderMachine, RecorderEffect) {
        confirmed_removal(RideRecoveryState::for_damage(damage))
    }

    /// One attempt per rider action, never one per pass. A fresh confirmation buys exactly one more
    /// attempt, with no reboot in between.
    #[test]
    fn a_failed_repair_costs_one_attempt_and_waits_for_the_rider() {
        let (mut rec, effect) = repairing(RideDamage::Payload);
        let verdict = rec.apply_outcome(RecorderOutcome::Failed { token: effect.token(), error: RecorderError::Write });
        assert_eq!(verdict, RecorderVerdict::RecoveryLatched, "the failure is latched, not re-offered");
        assert_eq!(rec.recovery(), RideRecoveryState::Latched(Some(RideDamage::Payload)));

        for pass in 0..5 {
            assert!(
                rec.next_effect(CAN_RECORD, at(2 + pass * CHECKPOINT_MS)).is_none(),
                "pass {pass}: a latched failure mints nothing — not the removal, and not a cadence either"
            );
        }

        rec.request(RecorderIntent::Discard);
        let retry = rec.next_effect(CAN_RECORD, at(60_000)).expect("a fresh confirmation is a fresh attempt");
        assert!(matches!(retry, RecorderEffect::Discard { .. }));
        assert!(rec.next_effect(CAN_RECORD, at(60_001)).is_none(), "and one is all it is");

        // An executor that abandoned the work without performing it costs the confirmation and
        // nothing else.
        rec.apply_outcome(RecorderOutcome::Cancelled { token: retry.token() });
        assert_eq!(rec.recovery(), RideRecoveryState::Repairable(RideDamage::Payload), "nothing was attempted");
        assert!(rec.next_effect(CAN_RECORD, at(60_002)).is_none(), "and the rider has not asked again");

        rec.request(RecorderIntent::Discard);
        let after_cancel = rec.next_effect(CAN_RECORD, at(60_003)).expect("a confirm after a cancel is honoured");
        assert!(matches!(after_cancel, RecorderEffect::Discard { .. }));
        assert!(rec.next_effect(CAN_RECORD, at(60_004)).is_none(), "and again, one is all it is");
    }

    /// A store that refuses every mutation is a different answer from a write that went wrong: the
    /// decision ends, for a damaged recording and a whole recovered ride alike.
    #[test]
    fn a_read_only_store_ends_the_decision_rather_than_offering_a_retry() {
        for offered in [RideRecoveryState::for_damage(RideDamage::Metadata), RideRecoveryState::Resumable] {
            let (mut rec, effect) = confirmed_removal(offered);
            let verdict =
                rec.apply_outcome(RecorderOutcome::Failed { token: effect.token(), error: RecorderError::ReadOnly });
            assert_eq!(verdict, RecorderVerdict::RecoveryLatched, "{offered:?}");
            assert_eq!(rec.recovery(), RideRecoveryState::Unrepairable, "{offered:?}");
            rec.request(RecorderIntent::Discard);
            assert!(
                rec.next_effect(CAN_RECORD, at(2)).is_none(),
                "{offered:?}: a store that takes no mutation is asked for none"
            );
        }
    }

    /// The bound is the rider's action, not the object's condition. Only the card differs: the
    /// terminal mode maps to "discard failed", and a `Cancelled` returns the rider to the full
    /// Continue/Discard card because the ride is still whole.
    #[test]
    fn a_failed_discard_of_a_whole_recovered_ride_costs_one_attempt_too() {
        let (mut rec, effect) = confirmed_removal(RideRecoveryState::Resumable);
        let verdict = rec.apply_outcome(RecorderOutcome::Failed { token: effect.token(), error: RecorderError::Write });
        assert_eq!(verdict, RecorderVerdict::RecoveryLatched, "no RecordingFailed, and no re-offer");
        assert_eq!(rec.recovery(), RideRecoveryState::Latched(None), "latched with no damage to name");

        for pass in 0..5 {
            assert!(
                rec.next_effect(CAN_RECORD, at(2 + pass * CHECKPOINT_MS)).is_none(),
                "pass {pass}: a latched discard re-attempts nothing"
            );
        }

        // The object still owns the store's ride state, so a Start opens no session either.
        rec.request(RecorderIntent::Start);
        assert_eq!(
            rec.advance(CAN_RECORD),
            RecorderAdvance::RecoveryOwed,
            "no phantom session behind a failed discard"
        );
        assert!(!rec.recording());

        rec.request(RecorderIntent::Discard);
        let retry = rec.next_effect(CAN_RECORD, at(60_000)).expect("a fresh confirmation is a fresh attempt");
        assert!(rec.next_effect(CAN_RECORD, at(60_001)).is_none(), "and one is all it is");

        rec.apply_outcome(RecorderOutcome::Cancelled { token: retry.token() });
        assert_eq!(rec.recovery(), RideRecoveryState::Resumable, "an untouched ride is still continuable");
        assert!(rec.next_effect(CAN_RECORD, at(60_002)).is_none(), "and the rider has not asked again");

        rec.continue_recovered();
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Opened(SessionStart::Recovered));
    }

    #[test]
    fn a_repaired_recording_lets_the_next_ride_open_in_the_same_boot() {
        let (mut rec, effect) = repairing(RideDamage::Payload);
        assert_eq!(rec.apply_outcome(RecorderOutcome::Discarded { token: effect.token() }), RecorderVerdict::Dropped);
        assert_eq!(rec.recovery(), RideRecoveryState::None, "the decision is over");

        rec.request(RecorderIntent::Start);
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Opened(SessionStart::Fresh), "recording is available now");
        assert!(rec.recording());
    }

    /// A catalog this executor could not read is one it never asks the store to mutate — not on the
    /// rider's confirmation, and not on any pass.
    #[test]
    fn an_unrepairable_recording_is_never_offered_to_the_store() {
        let mut rec = RecorderMachine::new();
        assert!(rec.offer_recovery(RideRecoveryState::for_damage(RideDamage::Catalog)));
        assert_eq!(rec.recovery(), RideRecoveryState::Unrepairable, "catalog damage has no repair");

        rec.request(RecorderIntent::Discard);
        for pass in 0..20 {
            assert!(
                rec.next_effect(CAN_RECORD, at(1 + pass * CHECKPOINT_MS)).is_none(),
                "pass {pass}: no effect is ever minted against an unreadable catalog"
            );
        }
    }

    #[test]
    fn a_start_against_a_damaged_recording_re_raises_the_decision_once_per_ask() {
        let mut rec = RecorderMachine::new();
        assert!(rec.offer_recovery(RideRecoveryState::for_damage(RideDamage::Payload)));

        rec.request(RecorderIntent::Start);
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::RecoveryOwed, "the decision is owed, not a session");
        assert!(!rec.recording(), "and nothing opened");
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Nothing, "one card per ask, not one per pass");

        rec.request(RecorderIntent::Start);
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::RecoveryOwed, "a fresh ask gets a fresh answer");

        // The Discard replaces the pending Start, so nothing auto-starts once the repair lands.
        rec.request(RecorderIntent::Discard);
        let effect = rec.next_effect(CAN_RECORD, at(1)).expect("the confirmed repair");
        rec.apply_outcome(RecorderOutcome::Discarded { token: effect.token() });
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Nothing, "no ride follows the repair on its own");
        assert!(!rec.recording());
    }

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

    #[test]
    fn staged_samples_survive_a_busy_append_slot() {
        let mut rec = recording();
        let checkpoint = rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS)).expect("the cadence came due");
        assert!(matches!(checkpoint, RecorderEffect::Checkpoint { .. }));

        rec.record_fix(Fix::at(BASE_LAT, LON), CHECKPOINT_MS, true);
        rec.record_fix(Fix::at(BASE_LAT + STEP_UD, LON), CHECKPOINT_MS + 1_000, true);
        assert!(rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS + 1_000)).is_none(), "one operation at a time");
        assert_eq!(rec.staged().len(), 2, "and the busy pass destroyed neither of them");

        rec.apply_outcome(RecorderOutcome::Checkpointed {
            token: checkpoint.token(),
            status: CheckpointStatus::Durable,
        });
        assert_eq!(append(&mut rec, CHECKPOINT_MS + 1_001, 2), 2, "both leave with the next append");
        assert!(rec.staged().is_empty());
    }

    #[test]
    fn a_partial_append_leaves_the_unwritten_samples_staged() {
        let mut rec = ridden(3);
        let third = rec.staged()[2];

        assert_eq!(append(&mut rec, 3_000, 1), 3, "three were offered");
        assert_eq!(rec.staged().len(), 2, "one reached the medium, two did not");

        let repair = rec.next_effect(CAN_RECORD, at(3_001)).unwrap();
        assert!(matches!(repair, RecorderEffect::Checkpoint { .. }));
        rec.apply_outcome(RecorderOutcome::Checkpointed { token: repair.token(), status: CheckpointStatus::Durable });
        assert_eq!(append(&mut rec, 3_002, 2), 2, "the retry offers exactly what is left");
        assert!(rec.staged().is_empty());
        assert_eq!(third.t_ms, 2_000, "and the tail of the batch is the tail of the ride");
    }

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
        let repair = rec.next_effect(CAN_RECORD, at(3_001)).unwrap();
        assert!(matches!(repair, RecorderEffect::Checkpoint { .. }));
        rec.apply_outcome(RecorderOutcome::Checkpointed { token: repair.token(), status: CheckpointStatus::Durable });
        assert_eq!(append(&mut rec, 3_002, 3), 3, "the retry is the same batch");
    }

    /// The blocked checkpoint keeps its samples staged during backoff, then outranks them when the
    /// recovery window ends.
    #[test]
    fn a_failed_checkpoint_holds_samples_until_its_successful_retry() {
        let mut rec = ridden(2);
        let staged = rec.staged().to_vec();
        let continuation = rec.continuation();
        let checkpoint = rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS)).expect("the cadence came due");
        assert!(matches!(checkpoint, RecorderEffect::Checkpoint { .. }), "{checkpoint:?}");
        let failed_at = CHECKPOINT_MS + 2_000;
        rec.apply_outcome_at(
            RecorderOutcome::Failed { token: checkpoint.token(), error: RecorderError::Write },
            failed_at,
        );

        assert!(rec.next_effect(CAN_RECORD, at(failed_at + CHECKPOINT_RETRY_MS - 1)).is_none());
        assert_eq!(rec.staged(), staged, "the accepted suffix stays staged during recovery");
        assert_eq!(rec.continuation(), continuation, "the accepted boundary stays unchanged");
        let retry = rec
            .next_effect(CAN_RECORD, at(failed_at + CHECKPOINT_RETRY_MS))
            .expect("the blocked journal owes its repair");
        assert!(matches!(retry, RecorderEffect::Checkpoint { .. }), "the repair still outranks the samples: {retry:?}");
        rec.apply_outcome(RecorderOutcome::Checkpointed { token: retry.token(), status: CheckpointStatus::Durable });
        assert_eq!(append(&mut rec, failed_at + CHECKPOINT_RETRY_MS + 1, 2), 2, "and the samples follow it");
    }

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

    #[test]
    fn a_fix_outside_a_ride_stages_nothing() {
        let mut rec = RecorderMachine::new();
        assert!(!rec.record_fix(Fix::at(BASE_LAT, LON), 0, true));
        assert!(rec.staged().is_empty(), "no session, no samples");
        assert!(!rec.breadcrumb.is_empty(), "but the trail is still the rider's");
    }

    #[test]
    fn a_new_session_clears_every_accumulator() {
        let mut rec = ridden(4);
        rec.record_hr(150, 4_000);
        rec.record_power(240, 4_000, true);
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

    #[test]
    fn save_repairs_and_drains_acknowledged_prefixes_before_finalizing() {
        let mut rec = ridden(3);
        let original = rec.staged().to_vec();
        let stats = rec.continuation();
        rec.request(RecorderIntent::Save);
        let first = rec.next_effect(CAN_RECORD, at(3_000)).unwrap();
        assert!(matches!(first, RecorderEffect::Append { samples: 3, .. }));
        rec.apply_outcome(RecorderOutcome::Appended { token: first.token(), samples: 1 });
        assert_eq!(rec.staged(), &original[1..]);
        // Delayed storage must not make Save acquire new samples or change its totals.
        rec.record_altitude(100.0, true);
        rec.record_fix(Fix::at(BASE_LAT + 10 * STEP_UD, LON), 4_000, true);
        assert_eq!(rec.continuation(), stats);
        assert_eq!(rec.staged(), &original[1..]);
        let repair = rec.next_effect(CAN_RECORD, at(4_000)).unwrap();
        assert!(matches!(repair, RecorderEffect::Checkpoint { .. }));
        rec.apply_outcome_at(RecorderOutcome::Failed { token: repair.token(), error: RecorderError::Write }, 8_000);
        assert!(rec.next_effect(CAN_RECORD, at(8_000 + CHECKPOINT_RETRY_MS - 1)).is_none());
        let retry = rec.next_effect(CAN_RECORD, at(8_000 + CHECKPOINT_RETRY_MS)).unwrap();
        assert!(matches!(retry, RecorderEffect::Checkpoint { .. }));
        // A stale append cannot consume the outstanding tail or the repair token.
        rec.apply_outcome(RecorderOutcome::Appended { token: first.token(), samples: 3 });
        assert_eq!(rec.staged(), &original[1..]);
        rec.apply_outcome(RecorderOutcome::Checkpointed { token: retry.token(), status: CheckpointStatus::Durable });
        let tail = rec.next_effect(CAN_RECORD, at(8_000 + CHECKPOINT_RETRY_MS + 1)).unwrap();
        assert!(matches!(tail, RecorderEffect::Append { samples: 2, .. }));
        rec.apply_outcome(RecorderOutcome::Appended { token: tail.token(), samples: 2 });
        let close = rec.next_effect(CAN_RECORD, at(8_000 + CHECKPOINT_RETRY_MS + 2)).unwrap();
        assert!(matches!(close, RecorderEffect::Finalize { .. }));
        rec.apply_outcome(RecorderOutcome::Failed { token: close.token(), error: RecorderError::Write });
        let retry = rec.next_effect(CAN_RECORD, at(8_000 + CHECKPOINT_RETRY_MS + 3)).unwrap();
        assert!(matches!(retry, RecorderEffect::Finalize { .. }));
        rec.apply_outcome(RecorderOutcome::Finalized { token: retry.token(), ride: 3 });
        assert!(!rec.recording());
    }

    #[test]
    fn batch_context_and_checkpoint_service_preserve_the_issued_boundary() {
        let mut rec = ridden(3);
        let effect = rec.next_effect(CAN_RECORD, at(4_000)).unwrap();
        let RecorderEffect::Append { samples, .. } = effect else { panic!("append owed") };
        assert_eq!(rec.append_context(samples), Some(rec.continuation()));
        assert_eq!(rec.checkpoint_context(), None);
        rec.record_fix(Fix::at(BASE_LAT + 4 * STEP_UD, LON), 4_000, true);
        assert_eq!(rec.append_context(samples), None, "later suffix must be reissued");
        let staged = rec.staged().to_vec();
        rec.apply_outcome(RecorderOutcome::Cancelled { token: effect.token() });
        let retry = rec.next_effect(CAN_RECORD, at(4_001)).unwrap();
        let RecorderEffect::Append { samples, .. } = retry else { panic!("retry append") };
        assert_eq!(usize::from(samples), staged.len());
        assert_eq!(rec.append_context(samples), Some(rec.continuation()));
        rec.apply_outcome(RecorderOutcome::NeedsCheckpoint { token: retry.token() });
        assert_eq!(rec.staged(), staged);
        let checkpoint = rec.next_effect(CAN_RECORD, at(4_002)).unwrap();
        assert!(matches!(checkpoint, RecorderEffect::Checkpoint { .. }));
        rec.apply_outcome(RecorderOutcome::Checkpointed {
            token: checkpoint.token(),
            status: CheckpointStatus::Unsupported,
        });
        let append = rec.next_effect(CAN_RECORD, at(4_003)).unwrap();
        assert!(matches!(append, RecorderEffect::Append { .. }));
        rec.apply_outcome(RecorderOutcome::Appended { token: append.token(), samples });
        assert_eq!(rec.checkpoint_context(), Some(rec.continuation()));
        rec.apply_outcome(RecorderOutcome::NeedsCheckpoint { token: retry.token() });
        assert!(rec.next_effect(CAN_RECORD, at(4_004)).is_none(), "stale refusal owes nothing");
    }

    #[test]
    fn save_after_failed_checkpoint_repairs_but_discard_bypasses_the_tail() {
        for discard in [false, true] {
            let mut rec = ridden(3);
            let checkpoint = rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS)).unwrap();
            assert!(matches!(checkpoint, RecorderEffect::Checkpoint { .. }));
            rec.apply_outcome(RecorderOutcome::Failed { token: checkpoint.token(), error: RecorderError::Write });
            rec.save_and_restart();
            if discard {
                rec.request(RecorderIntent::Discard);
            }
            let next_at = if discard { CHECKPOINT_MS + 1 } else { CHECKPOINT_MS + CHECKPOINT_RETRY_MS };
            if !discard {
                assert!(rec.next_effect(CAN_RECORD, at(next_at - 1)).is_none(), "Save waits for checkpoint recovery");
            }
            let next = rec.next_effect(CAN_RECORD, at(next_at)).unwrap();
            if discard {
                assert!(matches!(next, RecorderEffect::Discard { .. }));
                rec.apply_outcome(RecorderOutcome::Failed { token: next.token(), error: RecorderError::Write });
                assert_eq!(rec.staged().len(), 3);
                let retry = rec.next_effect(CAN_RECORD, at(CHECKPOINT_MS + 2)).unwrap();
                assert!(matches!(retry, RecorderEffect::Discard { .. }));
                rec.apply_outcome(RecorderOutcome::Discarded { token: retry.token() });
                assert!(rec.staged().is_empty());
                assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Nothing);
            } else {
                assert!(matches!(next, RecorderEffect::Checkpoint { .. }));
                rec.apply_outcome(RecorderOutcome::Checkpointed {
                    token: next.token(),
                    status: CheckpointStatus::Durable,
                });
                let append = rec.next_effect(CAN_RECORD, at(next_at + 1)).unwrap();
                assert!(matches!(append, RecorderEffect::Append { samples: 3, .. }));
                rec.apply_outcome(RecorderOutcome::Appended { token: append.token(), samples: 3 });
                let close = rec.next_effect(CAN_RECORD, at(next_at + 2)).unwrap();
                assert!(matches!(close, RecorderEffect::Finalize { .. }));
                rec.apply_outcome(RecorderOutcome::Finalized { token: close.token(), ride: 4 });
                assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Opened(SessionStart::Fresh));
            }
        }
    }

    /// The two halves run in one pass, so without the integration anchors going with the totals the
    /// new ride would book the step between the rides as its own.
    #[test]
    fn save_and_restart_does_not_carry_the_old_rides_anchor_into_the_new_one() {
        let mut rec = ridden(3);
        rec.save_and_restart();
        let effect = rec.next_effect(CAN_RECORD, at(2_000)).expect("the close goes first");
        rec.apply_outcome(RecorderOutcome::Finalized { token: effect.token(), ride: 1 });
        // The pass's two session edges, in the order it runs them.
        rec.reset_totals();
        rec.restart_buffers();
        assert_eq!(rec.advance(CAN_RECORD), RecorderAdvance::Opened(SessionStart::Fresh));
        rec.reset_totals();
        rec.restart_buffers();

        // Ride two's first fix: one second later and ~5 m on from where ride one ended.
        rec.record_fix(Fix::at(BASE_LAT + 3 * STEP_UD, LON), 3_000, true);
        assert_eq!(rec.ridden_m(), 0.0, "the step between the two rides belongs to neither");
        assert_eq!(rec.moving_s(), 0.0, "and it books no moving time either");
        assert!(rec.staged()[0].segment_start, "ride two's first sample opens ride two's segment");
    }

    #[test]
    fn one_hz_fix_stream_integrates_without_teleport_rejection() {
        let rec = ridden(5);
        assert!((16.0..=24.0).contains(&rec.ridden_m()), "four ~5 m steps ≈ 20 m, got {}", rec.ridden_m());
        assert_eq!(rec.moving_s(), 4.0, "every 1 s interval counts toward moving time");
        let avg = rec.avg_kmh().expect("moving time accrued");
        assert!((10.0..=25.0).contains(&avg), "~18 km/h, got {avg}");
        assert_eq!(rec.staged().len(), 5, "and every one of them is logged");
    }

    /// A stopped rider still emits fresh, identical-position fixes.
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

    /// The duplicate logs nothing and arms no segment break, so the next real fix integrates
    /// normally rather than being rejected as an infinite-speed teleport.
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

    #[test]
    fn a_paused_ride_drops_its_anchor_and_books_nothing() {
        let mut rec = recording();
        rec.record_fix(Fix::at(BASE_LAT, LON), 0, false);
        rec.record_fix(Fix::at(BASE_LAT + STEP_UD, LON), 1000, false);
        assert_eq!(rec.ridden_m(), 0.0);
        assert!(rec.staged().is_empty(), "a paused ride logs nothing");
    }

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

    #[test]
    fn motion_near_the_pole_shrinks_longitude_distance() {
        let mut rec = recording();
        rec.record_fix(Fix::at(85_000_000, 0), 0, true);
        rec.record_fix(Fix::at(85_000_000, 100), 1000, true);
        assert!(rec.ridden_m() < 3.0, "heavily foreshortened at 85°N, got {}", rec.ridden_m());
        assert!(rec.ridden_m() > 0.0, "but still a real, non-zero step");
    }

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

    #[test]
    fn a_stale_strap_records_absent_not_its_last_value() {
        let mut rec = recording();
        rec.record_fix(Fix::at(BASE_LAT, LON), 0, true);
        rec.record_hr(150, 1_000);
        rec.record_power(200, 1_000, true);
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

    #[test]
    fn kj_counts_power_samples_and_a_dropout_adds_at_most_the_gap_cap() {
        let mut rec = recording();
        assert_eq!(rec.kj(), None, "no power data, no energy");
        // No fix at all: energy does not wait for GPS or for movement.
        for s in 0..=10 {
            rec.record_power(1_000, s * 1_000, true);
        }
        assert_eq!(rec.kj(), Some(10), "ten one-second intervals at 1000 W");

        // The meter drops out for 30 s. The first sample after it is credited for 2 s, not 30.
        rec.record_power(1_000, 40_000, true);
        assert_eq!(rec.kj(), Some(12), "the gap adds the 2 s cap and nothing more");

        // Paused: samples keep the live value, but the ride books no energy.
        rec.record_power(1_000, 41_000, false);
        assert_eq!(rec.kj(), Some(12));
    }

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

    #[test]
    fn a_recovered_continuation_restores_the_raw_summary_state() {
        let state = RideContinuation {
            origin: RideOrigin {
                bike: BikeType::Mtb,
                trip: TripRef::new(9, 1, 3),
                limits: EffortLimits { max_hr: 185, ftp_w: 250 },
            },
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
            energy_j: Some(19_600),
        };
        let mut rec = RecorderMachine::new();
        rec.restore_continuation(state);
        assert_eq!(rec.continuation(), state);
        assert_eq!((rec.avg_hr(), rec.avg_power(), rec.avg_cadence()), (Some(150), Some(245), Some(87)));
        assert_eq!(rec.kj(), Some(19), "the ride's energy crosses the reset");
        assert_eq!((rec.ride_stats().energy_kj, rec.ride_stats().descent_m), (Some(19), 123), "and reach the footer");
        assert_eq!(rec.climb_m(), 321.0);
        assert_eq!((rec.ride_stats().bike, rec.ride_stats().trip), (BikeType::Mtb, TripRef::new(9, 1, 3)));
        assert_eq!(
            rec.ride_stats().limits,
            EffortLimits { max_hr: 185, ftp_w: 250 },
            "a resumed ride keeps its limits"
        );
    }

    #[test]
    fn the_footer_facts_come_from_recorder_at_finalize() {
        let mut rec = ridden(5);
        rec.record_hr(150, 4_000);
        rec.record_fix(Fix::at(BASE_LAT + 5 * STEP_UD, LON), 5_000, true);
        rec.record_altitude(100.0, true);
        rec.record_altitude(140.0, true);

        rec.request(RecorderIntent::Save);
        let clock = FooterClock { unix_at_anchor: 1_720_000_500, anchor_ms: 5_000, trusted: true };
        let drain = rec.next_effect(CAN_RECORD, clock).unwrap();
        assert!(matches!(drain, RecorderEffect::Append { samples: 6, .. }));
        rec.apply_outcome(RecorderOutcome::Appended { token: drain.token(), samples: 6 });
        let effect = rec.next_effect(CAN_RECORD, clock).expect("the drained Save can finalize");
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
