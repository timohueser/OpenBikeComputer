//! Flat-store ride recording: the board's half of the Recorder protocol.
//!
//! [`RecorderMachine`](obc_app::RecorderMachine) decides what a ride is and when it closes, and this
//! module performs the physical operations that decision names: start one `RECORDING` object,
//! collect the 20-byte sample bytes, checkpoint them through the tail journal, append one footer,
//! and clear `RECORDING` in one commit. There is no temporary file and no finish-time conversion,
//! and no lifecycle rule here: one method per `RecorderEffect`, each answering whether the store did
//! it.

use core::ptr::{addr_of, addr_of_mut};

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use heapless::Vec;
use obc_crc::Crc32;
use obc_formats::ride::{decode_footer, FOOTER_LEN, SAMPLE_LEN};
use obc_ports::TrackPoint;
use obc_storage::flat::{
    DisplayName, EntryFlags, EntryMeta, FlatStore, Mutation, ObjectId, ObjectKind, PutSource, Revision, RideCheckpoint,
    Store as _, StoreError, RIDE_RESUME_LEN,
};

use obc_app::recorder::{CheckpointStatus, RecorderEffect, RecorderError, RecorderOutcome, RideClose};
use obc_app::RideDamage;

use crate::flat_store::{FlatCard, Outcome, Reply, Request, Writer};

const RIDE_RESERVE: u64 = 32 * 1024 * 1024;
// At the minimum 1 s fix cadence, one checkpoint interval contributes at most ten records. Six
// extra records cover a delayed pass, and the footer always keeps its own reserved space. The store
// owns the durable partial page; the board retains only bytes appended since the last successful
// logical checkpoint.
const DELTA_SAMPLES: usize = 16;
const DELTA_BYTES: usize = DELTA_SAMPLES * SAMPLE_LEN + FOOTER_LEN;

static REPLY: Reply = Signal::<CriticalSectionRawMutex, _>::new();
static mut DELTA: [u8; DELTA_BYTES] = [0; DELTA_BYTES];
static mut RESUME: [u8; RIDE_RESUME_LEN] = [0; RIDE_RESUME_LEN];

use obc_app::recorder::continuation::{decode as decode_resume, encode as encode_resume};
const _: () = assert!(RIDE_RESUME_LEN == obc_app::recorder::continuation::RIDE_RESUME_LEN);

#[derive(Clone, Copy)]
struct ClockRebase {
    source_anchor: u32,
    logical_anchor: u32,
}

#[derive(Clone, Copy)]
struct Live {
    session: Option<u32>,
    id: ObjectId,
    revision: Revision,
    name: DisplayName,
    delta_len: usize,
    points: u32,
    first_t_ms: Option<u32>,
    last_t_ms: Option<u32>,
    /// Stable UTC start, persisted with every logical checkpoint. A same-boot short ride can derive
    /// it at Finish, but a continued ride must never combine an old monotonic sample with a new
    /// boot's wall-clock anchor.
    start_time: Option<u32>,
    /// Only a ride still in the boot where its first sample was recorded may derive a trusted UTC
    /// start later. Once recovered, `first_t_ms` belongs to the old monotonic domain, and combining
    /// it with this boot's wall anchor would back-date the ride by nearly 2^32 ms.
    can_upgrade_start: bool,
    clock_rebase: Option<ClockRebase>,
    continuation: obc_app::RideContinuation,
    /// A failed checkpoint blocks further samples until the exact same append has been retried. That
    /// preserves storage's gated rollover recovery anchor and bounds loss under repeated media
    /// faults, instead of letting the caller mutate the retry payload.
    journal_blocked: bool,
    crc: Crc32,
    last_checkpoint_ms: u32,
}

#[derive(Clone, Copy)]
struct Finalising {
    id: ObjectId,
    revision: Revision,
    name: DisplayName,
    delta_len: usize,
    payload_len: u64,
    payload_crc: u32,
    journaled: bool,
}

#[derive(Clone, Copy)]
enum State {
    Idle,
    Live(Live),
    /// Finish was requested while an ordinary checkpoint was blocked. The footer is staged after
    /// `live.delta_len` but is not yet part of that length or CRC: storage must first see the exact
    /// failed append and resume again. Once the repair succeeds the footer moves to offset zero and
    /// the normal final checkpoint publishes it.
    FinaliseAfterRepair(Live),
    /// A durable `RECORDING` object this executor could not attach to a session, carrying which of
    /// the three refusals produced it. It stays visible to recovery diagnostics, is never appended
    /// to, and its only exit is the rider-confirmed exact removal.
    Faulted {
        id: ObjectId,
        revision: Revision,
        damage: RideDamage,
    },
    Finalising(Finalising),
    Discarding {
        id: ObjectId,
        revision: Revision,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppendResult {
    Accepted,
    NeedsCheckpoint,
    Failed,
}

pub(crate) struct Recorder {
    state: State,
    writer: Writer,
    warning_pending: bool,
    /// `debug-uart` only: the next exact removal answers as a media failure would, without issuing
    /// the commit. A genuine media fault cannot be produced safely on a working board, and the
    /// executor's answer is exactly where a real one surfaces.
    #[cfg(feature = "debug-uart")]
    repair_fail_once: bool,
}

impl Recorder {
    pub(crate) fn new(store: &'static FlatStore<FlatCard>, writer: Writer, now_ms: u32) -> Self {
        let state = match store.recovered_ride() {
            None => State::Idle,
            Some(recovered) => {
                let total = recovered.payload_len();
                let first_t_ms = read_sample_time(store, 0, total);
                let last_t_ms = total.checked_sub(SAMPLE_LEN as u64).and_then(|at| read_sample_time(store, at, total));
                let mut catalog_name = None;
                for entry in store.entries() {
                    if (entry.id, entry.revision) == (recovered.id, recovered.revision) {
                        catalog_name = Some(entry.name);
                    }
                }
                if !store.entries_ok() || catalog_name.is_none() {
                    defmt::error!("flat ride: recovered catalog entry could not be read completely");
                    return Self {
                        state: faulted(recovered.id, recovered.revision, RideDamage::Catalog),
                        writer,
                        // The card names the damage. A recording warning beside it would claim a
                        // ride log went incomplete, which is not what happened.
                        warning_pending: false,
                        #[cfg(feature = "debug-uart")]
                        repair_fail_once: false,
                    };
                }
                let catalog_name = catalog_name.unwrap_or_default();

                // A footer-bearing checkpoint means Finish made the final bytes durable and only the
                // clearing commit was cut. Validate the footer before completing it; otherwise the
                // payload stays a resumable sequence of exact 20-byte samples.
                if total >= FOOTER_LEN as u64 && (total - FOOTER_LEN as u64).is_multiple_of(SAMPLE_LEN as u64) {
                    let mut bytes = [0u8; FOOTER_LEN];
                    let at = total - FOOTER_LEN as u64;
                    if store.read_recovered(at, &mut bytes).is_ok() {
                        if let Ok(footer) = decode_footer(&bytes) {
                            let points = ((total - FOOTER_LEN as u64) / SAMPLE_LEN as u64) as u32;
                            if footer.point_count == points {
                                let name = DisplayName::new(footer.name()).unwrap_or_default();
                                return Self {
                                    state: State::Finalising(Finalising {
                                        id: recovered.id,
                                        revision: recovered.revision,
                                        name,
                                        delta_len: 0,
                                        payload_len: total,
                                        payload_crc: recovered.payload_crc,
                                        journaled: true,
                                    }),
                                    writer,
                                    warning_pending: false,
                                    #[cfg(feature = "debug-uart")]
                                    repair_fail_once: false,
                                };
                            }
                        }
                    }
                }

                let sample_anchors_valid = total == 0 || (first_t_ms.is_some() && last_t_ms.is_some());
                if total.is_multiple_of(SAMPLE_LEN as u64) && sample_anchors_valid {
                    let initial = total == 0
                        && recovered.checkpoint_sequence == 0
                        && recovered.resume.iter().all(|byte| *byte == 0);
                    let resumed = if initial {
                        Some((obc_app::RideContinuation::default(), None))
                    } else {
                        decode_resume(&recovered.resume)
                    };
                    let Some((continuation, start_time)) = resumed else {
                        defmt::error!("flat ride: recovered samples have no valid continuation metadata");
                        return Self {
                            state: faulted(recovered.id, recovered.revision, RideDamage::Metadata),
                            writer,
                            warning_pending: false,
                            #[cfg(feature = "debug-uart")]
                            repair_fail_once: false,
                        };
                    };
                    State::Live(Live {
                        session: None,
                        id: recovered.id,
                        revision: recovered.revision,
                        name: catalog_name,
                        delta_len: 0,
                        points: (total / SAMPLE_LEN as u64) as u32,
                        first_t_ms,
                        last_t_ms,
                        start_time,
                        can_upgrade_start: false,
                        clock_rebase: None,
                        continuation,
                        journal_blocked: false,
                        crc: Crc32::from_checksum(recovered.payload_crc),
                        last_checkpoint_ms: now_ms,
                    })
                } else {
                    // The store proved a durable checkpoint, but it is not a sample or footer
                    // boundary. Keep the RECORDING object intact and loud, and never append to or
                    // publish bytes whose format this executor cannot prove.
                    defmt::error!("flat ride: recovered payload is not a v3 sample/footer boundary");
                    faulted(recovered.id, recovered.revision, RideDamage::Payload)
                }
            }
        };
        Recorder {
            state,
            writer,
            warning_pending: false,
            #[cfg(feature = "debug-uart")]
            repair_fail_once: false,
        }
    }

    /// Whether a durable `RECORDING` object is open, closing or faulted: the DFU arm's refusal
    /// check.
    pub(crate) fn is_recording(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    /// The ride session the live object belongs to, or `None` when no object is attached to one. A
    /// faulted or closing object answers `None`: neither is a session's to record into.
    pub(crate) fn open_session(&self) -> Option<u32> {
        match self.state {
            State::Live(live) => live.session,
            _ => None,
        }
    }

    /// The app-side state paired with a recovered logical checkpoint. The board restores this before
    /// showing the Continue or Discard card, so malformed metadata never reaches the UI.
    pub(crate) fn recovered_continuation(&self) -> Option<obc_app::RideContinuation> {
        let State::Live(Live { session: None, continuation, .. }) = self.state else { return None };
        Some(continuation)
    }

    /// What is wrong with the recovered object, or `None`. The app decides from this whether a
    /// repair may be offered at all.
    pub(crate) fn recovery_damage(&self) -> Option<RideDamage> {
        match self.state {
            State::Faulted { damage, .. } => Some(damage),
            _ => None,
        }
    }

    pub(crate) fn take_warning(&mut self) -> bool {
        core::mem::take(&mut self.warning_pending)
    }

    /// Service a terminal state left over from a reset: a footer-bearing recovered object whose
    /// clearing commit was cut. Run once at boot, before the first UI pass.
    pub(crate) async fn settle(&mut self) {
        let _ = self.service_terminal().await;
    }

    /// Open a ride object for `session`, saved as `name`.
    ///
    /// A recovered object with no session attached adopts this one instead of starting a second.
    /// That is what continuing means, and it is the only way the restored samples keep their object.
    pub(crate) async fn open(&mut self, store: &'static FlatStore<FlatCard>, session: u32, name: &str, now_ms: u32) {
        match self.state {
            State::Idle => {
                if let Err(error) = self.start(store, Some(session), name, now_ms).await {
                    self.warning_pending = true;
                    defmt::warn!("flat ride: start failed: {}", defmt::Debug2Format(&error));
                }
            }
            State::Live(mut live) if live.session.is_none() => {
                live.session = Some(session);
                if live.name.is_empty() {
                    live.name = DisplayName::new(name).unwrap_or_default();
                }
                live.last_checkpoint_ms = now_ms;
                if let Some(logical_anchor) = live.last_t_ms {
                    live.clock_rebase = Some(ClockRebase { source_anchor: now_ms, logical_anchor });
                }
                self.state = State::Live(live);
                defmt::info!("flat ride: continuing recovered object {=u64}", live.id.0);
            }
            _ => {}
        }
    }

    /// Open the owed session before serving its effect, including samples from the Start pass. Keep
    /// the opened identity after a close: its outcome reaches App on the next pass.
    pub(crate) async fn execute(
        &mut self,
        store: &'static FlatStore<FlatCard>,
        app: &obc_app::App,
        opened_session: &mut Option<u32>,
        effect: Option<RecorderEffect>,
        now: u32,
    ) -> Option<RecorderOutcome> {
        if let Some(id) = app.recorder.object_owed(*opened_session) {
            let name =
                app.active_route_index().and_then(|i| app.routes().get(i)).map_or("", |route| route.name.as_str());
            self.open(store, id, name, now).await;
            // A refused start stays owed and is retried on the next iteration.
            if self.open_session() == Some(id) {
                *opened_session = Some(id);
            }
        }
        Some(match effect? {
            RecorderEffect::Checkpoint { token } => {
                let stats = app.recorder.ride_stats();
                let continuation = app.recorder.checkpoint_context();
                match self.checkpoint(now, &stats, continuation).await {
                    Ok(status) => RecorderOutcome::Checkpointed { token, status },
                    Err(error) => RecorderOutcome::Failed { token, error },
                }
            }
            RecorderEffect::Finalize { token } => {
                // Recorder has already drained the samples through acknowledged appends, and the
                // footer facts come from Recorder, which stamped its wall-clock anchor as it minted
                // this close. The save name is not read at all: it was frozen when the ride opened.
                let stats = app.recorder.ride_stats();
                match self.finalize(&stats).await {
                    RideClose::Committed(ride) => RecorderOutcome::Finalized { token, ride },
                    RideClose::Nothing => {
                        defmt::warn!("flat ride: finalize with no open object — the ride was never created");
                        RecorderOutcome::Discarded { token }
                    }
                    RideClose::Failed => RecorderOutcome::Failed { token, error: RecorderError::Write },
                }
            }
            // The store's refusal is reported by kind. A card that will take no mutation at all is
            // the one answer a retry cannot help, so Recorder must be able to tell it from a write
            // that went wrong.
            RecorderEffect::Discard { token } => match self.discard().await {
                Ok(()) => RecorderOutcome::Discarded { token },
                Err(StoreError::ReadOnly) => RecorderOutcome::Failed { token, error: RecorderError::ReadOnly },
                Err(_) => RecorderOutcome::Failed { token, error: RecorderError::Write },
            },
            // The immutable App borrow binds the full issued batch to its observation context. A
            // changed cohort is reissued before any board storage work.
            RecorderEffect::Append { token, samples } => match app.recorder.append_context(samples) {
                None => RecorderOutcome::Cancelled { token },
                Some(context) => match self.append(app.recorder.staged(), context) {
                    AppendResult::Accepted => RecorderOutcome::Appended { token, samples },
                    AppendResult::NeedsCheckpoint => RecorderOutcome::NeedsCheckpoint { token },
                    AppendResult::Failed => RecorderOutcome::Failed { token, error: RecorderError::Write },
                },
            },
        })
    }

    /// Close the ride into a durable ride object.
    ///
    /// [`RideClose::Failed`](obc_app::recorder::RideClose) leaves the object `RECORDING` on the card
    /// with its footer staged, and Recorder re-offers the same finalize, which re-enters the
    /// terminal service below at exactly the step that failed. `Nothing` is the honest answer when
    /// no object was ever created: a start this card refused already warned the rider.
    ///
    /// The save name is not a parameter: it was frozen when the ride opened, so a mid-ride route
    /// swap cannot rename a ride that is already recording.
    pub(crate) async fn finalize(&mut self, stats: &obc_route::RideStats) -> obc_app::recorder::RideClose {
        use obc_app::recorder::RideClose;
        if let State::Live(live) = self.state {
            if live.journal_blocked {
                self.stage_finalise_after_repair(live, stats);
            } else {
                self.begin_finalise(live, stats);
            }
        }
        let closing = match self.state {
            State::Finalising(f) => f.id.0,
            State::FinaliseAfterRepair(live) => live.id.0,
            State::Idle => return RideClose::Nothing,
            // A faulted or discarding object is not this close's to commit.
            _ => return RideClose::Failed,
        };
        let _ = self.service_terminal().await;
        match self.state {
            State::Idle => RideClose::Committed(closing),
            _ => RideClose::Failed,
        }
    }

    /// Delete the open ride and its journal, and say why if the store refused. The typed error is
    /// what separates a write that went wrong from a store that will take no mutation at all.
    ///
    /// Deletion deliberately does not repair a blocked checkpoint. `Remove` never enters storage's
    /// final-tail flush: the atomic catalog mutation first makes the ride unreachable, then
    /// `settle_ride` clears its pending recovery state. Repair would add fallible I/O to an object
    /// the rider explicitly asked to destroy, which is exactly why a damaged object can be removed
    /// when it can be repaired no other way.
    pub(crate) async fn discard(&mut self) -> Result<(), StoreError> {
        match self.state {
            State::Live(live) | State::FinaliseAfterRepair(live) => {
                self.state = State::Discarding { id: live.id, revision: live.revision };
            }
            State::Faulted { id, revision, .. } => self.state = State::Discarding { id, revision },
            _ => {}
        }
        if let State::Discarding { id, .. } = self.state {
            defmt::info!("flat ride: rider-confirmed exact removal of {=u64}", id.0);
        }
        #[cfg(feature = "debug-uart")]
        if core::mem::take(&mut self.repair_fail_once) && matches!(self.state, State::Discarding { .. }) {
            // No commit is issued, so the card is untouched: the injection sits exactly where a real
            // media failure would reach Recorder. The state stays `Discarding`, so the rider's retry
            // re-attempts the very same removal.
            defmt::warn!("flat ride: exact removal refused: {}", defmt::Debug2Format(&StoreError::Media));
            return Err(StoreError::Media);
        }
        match self.service_terminal().await {
            Ok(()) => {
                defmt::info!("flat ride: exact removal committed");
                Ok(())
            }
            Err(error) => {
                defmt::warn!("flat ride: exact removal refused: {}", defmt::Debug2Format(&error));
                Err(error)
            }
        }
    }

    /// Admit the whole issued batch and its App observation boundary together. A capacity refusal
    /// leaves the previous bytes, CRC, times, point count and continuation untouched.
    pub(crate) fn append(&mut self, points: &[TrackPoint], continuation: obc_app::RideContinuation) -> AppendResult {
        let State::Live(mut live) = self.state else { return AppendResult::Failed };
        if live.session.is_none() || points.len() > DELTA_SAMPLES {
            return AppendResult::Failed;
        }
        let Some(count) = live.points.checked_add(points.len() as u32) else { return AppendResult::Failed };
        if live.journal_blocked || live.delta_len + points.len() * SAMPLE_LEN + FOOTER_LEN > DELTA_BYTES {
            return AppendResult::NeedsCheckpoint;
        }
        for point in points {
            let mut point = *point;
            if let Some(clock) = live.clock_rebase {
                point.t_ms = clock.logical_anchor.wrapping_add(point.t_ms.wrapping_sub(clock.source_anchor));
            }
            let sample = obc_formats::track::encode_record(&point);
            unsafe { delta_mut()[live.delta_len..live.delta_len + SAMPLE_LEN].copy_from_slice(&sample) };
            live.delta_len += SAMPLE_LEN;
            live.first_t_ms.get_or_insert(point.t_ms);
            live.last_t_ms = Some(point.t_ms);
            live.crc.update(&sample);
        }
        live.points = count;
        live.continuation = continuation;
        self.state = State::Live(live);
        AppendResult::Accepted
    }

    /// Checkpoint the accepted boundary. A current continuation is supplied only when App staging is
    /// empty. A failed attempt always replays its frozen tuple before considering fresh context.
    pub(crate) async fn checkpoint(
        &mut self,
        now_ms: u32,
        stats: &obc_route::RideStats,
        continuation: Option<obc_app::RideContinuation>,
    ) -> Result<CheckpointStatus, RecorderError> {
        let State::Live(live) = self.state else { return Err(RecorderError::Write) };
        // Once an attempt fails, storage's equality contract requires the entire logical checkpoint
        // to be replayed: append, CRC and opaque resume. App totals can keep moving while samples
        // are frozen, so never rebuild resume from the current app on a retry.
        let (resume, attempted_continuation, attempted_start) = if live.journal_blocked {
            (unsafe { *resume_slice() }, live.continuation, live.start_time)
        } else {
            let stable_start = live.start_time.or_else(|| {
                (live.can_upgrade_start && stats.clock_trusted).then(|| start_time(stats, live.first_t_ms))
            });
            let accepted = continuation.unwrap_or(live.continuation);
            (encode_resume(accepted, stable_start), accepted, stable_start)
        };
        match self.journal(live, &resume).await {
            Ok(()) => {
                let mut next = live;
                unsafe { delta_mut()[..next.delta_len].fill(0) };
                next.delta_len = 0;
                next.last_checkpoint_ms = now_ms;
                next.start_time = attempted_start;
                next.continuation = attempted_continuation;
                next.journal_blocked = false;
                self.state = State::Live(next);
                Ok(CheckpointStatus::Durable)
            }
            Err(error) => {
                let mut blocked = live;
                // These now name the staged resume, not the last durable one. Keeping them beside
                // `journal_blocked` makes a later successful retry advance the in-RAM state to the
                // snapshot storage just accepted, rather than to newer app totals.
                blocked.start_time = attempted_start;
                blocked.continuation = attempted_continuation;
                blocked.journal_blocked = true;
                self.state = State::Live(blocked);
                self.warning_pending = true;
                defmt::warn!("flat ride: checkpoint failed: {}", defmt::Debug2Format(&error));
                Err(RecorderError::Write)
            }
        }
    }

    fn stage_finalise_after_repair(&mut self, live: Live, stats: &obc_route::RideStats) {
        let mut footer_stats = *stats;
        let stable_start = live
            .start_time
            .or_else(|| (live.can_upgrade_start && stats.clock_trusted).then(|| start_time(stats, live.first_t_ms)))
            .unwrap_or(0);
        footer_stats.unix_at_anchor = stable_start;
        footer_stats.anchor_ms = live.first_t_ms.unwrap_or(0);
        footer_stats.clock_trusted = live.start_time.is_some() || (live.can_upgrade_start && stats.clock_trusted);
        let footer = obc_route::encode_summary_footer(
            live.name.as_str().unwrap_or(""),
            &footer_stats,
            live.points,
            live.first_t_ms,
        );
        if live.delta_len + footer.len() > DELTA_BYTES {
            self.warning_pending = true;
            defmt::error!("flat ride: staged footer does not fit after blocked checkpoint delta");
            return;
        }
        unsafe { delta_mut()[live.delta_len..live.delta_len + footer.len()].copy_from_slice(&footer) };
        self.state = State::FinaliseAfterRepair(live);
    }

    fn begin_finalise(&mut self, live: Live, stats: &obc_route::RideStats) {
        let name = live.name;
        let mut footer_stats = *stats;
        // A trusted checkpoint wins permanently. A fresh same-boot ride may still acquire UTC at
        // Finish; a recovered ride without a trusted checkpoint cannot mix its old monotonic
        // first-sample timestamp with this boot's wall anchor, and therefore reports start 0.
        let stable_start = live
            .start_time
            .or_else(|| (live.can_upgrade_start && stats.clock_trusted).then(|| start_time(stats, live.first_t_ms)))
            .unwrap_or(0);
        footer_stats.unix_at_anchor = stable_start;
        footer_stats.anchor_ms = live.first_t_ms.unwrap_or(0);
        footer_stats.clock_trusted = live.start_time.is_some() || (live.can_upgrade_start && stats.clock_trusted);
        let footer =
            obc_route::encode_summary_footer(name.as_str().unwrap_or(""), &footer_stats, live.points, live.first_t_ms);
        if live.delta_len + footer.len() > DELTA_BYTES {
            defmt::error!("flat ride: footer does not fit the bounded delta");
            return;
        }
        unsafe { delta_mut()[live.delta_len..live.delta_len + footer.len()].copy_from_slice(&footer) };
        let mut crc = live.crc;
        crc.update(&footer);
        self.state = State::Finalising(Finalising {
            id: live.id,
            revision: live.revision,
            name,
            delta_len: live.delta_len + footer.len(),
            payload_len: u64::from(live.points) * SAMPLE_LEN as u64 + FOOTER_LEN as u64,
            payload_crc: crc.finalize(),
            journaled: false,
        });
    }

    async fn start(
        &mut self,
        store: &'static FlatStore<FlatCard>,
        session: Option<u32>,
        name: &str,
        now_ms: u32,
    ) -> Result<(), StoreError> {
        let allocation = match self.writer.call(Request::Allocate { bytes: RIDE_RESERVE }, &REPLY).await? {
            Outcome::Allocated(allocation) => allocation,
            _ => return Err(StoreError::Invalid),
        };
        let id = store.next_object_id();
        let revision = Revision(1);
        let name = DisplayName::new(name).unwrap_or_default();
        let meta = EntryMeta {
            added_at_utc: 0,
            id,
            revision,
            kind: ObjectKind::Ride,
            flags: EntryFlags::RECORDING,
            payload_len: 0,
            payload_crc: 0,
            name,
        };
        let mut batch = Vec::new();
        batch.push(Mutation::Put { meta, source: PutSource::Fresh(allocation) }).map_err(|_| StoreError::Invalid)?;
        match self.writer.call(Request::Commit { batch }, &REPLY).await {
            Ok(Outcome::Committed(_)) => {
                unsafe { delta_mut().fill(0) };
                self.state = State::Live(Live {
                    session,
                    id,
                    revision,
                    name,
                    delta_len: 0,
                    points: 0,
                    first_t_ms: None,
                    last_t_ms: None,
                    start_time: None,
                    can_upgrade_start: true,
                    clock_rebase: None,
                    continuation: obc_app::RideContinuation::default(),
                    journal_blocked: false,
                    crc: Crc32::new(),
                    last_checkpoint_ms: now_ms,
                });
                Ok(())
            }
            Ok(_) => Err(StoreError::Invalid),
            Err(error) => {
                let _ = self.writer.call(Request::Cancel { allocation }, &REPLY).await;
                Err(error)
            }
        }
    }

    /// `debug-uart` only: fabricate a damaged `RECORDING` object through the production seam — one
    /// `start` and one journalled checkpoint whose bytes fail exactly one of the two logical
    /// recovery checks at the next mount. No card surgery and no block editing.
    #[cfg(feature = "debug-uart")]
    pub(crate) async fn debug_fabricate_damage(
        &mut self,
        store: &'static FlatStore<FlatCard>,
        kind: obc_platform::debug_link::RideDamageKind,
        now_ms: u32,
    ) {
        use obc_platform::debug_link::RideDamageKind;
        if !matches!(self.state, State::Idle) {
            defmt::warn!("flat ride: ride-damage refused -- an object is already open");
            return;
        }
        if let Err(error) = self.start(store, None, "DAMAGED", now_ms).await {
            defmt::error!("flat ride: ride-damage could not open an object: {}", defmt::Debug2Format(&error));
            return;
        }
        let State::Live(mut live) = self.state else { return };
        let (len, resume, name) = match kind {
            // 7 is below `FOOTER_LEN` and is not a whole number of samples, so the mount takes the
            // sample and footer boundary refusal with a resume image that would otherwise decode.
            RideDamageKind::Payload => (7usize, encode_resume(obc_app::RideContinuation::default(), None), "payload"),
            // One whole sample keeps the boundary valid, so the refusal is the resume image's, which
            // is what separates this cause from the first.
            RideDamageKind::Metadata => (SAMPLE_LEN, [0u8; RIDE_RESUME_LEN], "metadata"),
        };
        unsafe { delta_mut()[..len].fill(0) };
        live.delta_len = len;
        live.crc.update(unsafe { delta_slice(len) });
        match self.journal(live, &resume).await {
            Ok(()) => {
                live.delta_len = 0;
                self.state = State::Live(live);
                defmt::info!("flat ride: fabricated a damaged RECORDING {=u64} ({=str})", live.id.0, name);
            }
            Err(error) => {
                defmt::error!("flat ride: ride-damage journal failed: {}", defmt::Debug2Format(&error));
            }
        }
    }

    /// `debug-uart` only: arm the one-shot refusal of the next exact removal.
    #[cfg(feature = "debug-uart")]
    pub(crate) fn debug_arm_repair_failure(&mut self) {
        self.repair_fail_once = true;
        defmt::info!("flat ride: the next exact removal will be refused as a media failure");
    }

    async fn journal(&self, live: Live, resume: &[u8; RIDE_RESUME_LEN]) -> Result<(), StoreError> {
        unsafe { resume_mut().copy_from_slice(resume) };
        let checkpoint = RideCheckpoint {
            id: live.id,
            revision: live.revision,
            append: unsafe { delta_slice(live.delta_len) },
            payload_crc: live.crc.finalize(),
            resume: unsafe { resume_slice() },
        };
        match self.writer.call(Request::Journal { checkpoint }, &REPLY).await? {
            Outcome::Done => Ok(()),
            _ => Err(StoreError::Invalid),
        }
    }

    /// Run the state's own next step and say what the store answered.
    ///
    /// The typed error is load-bearing for exactly one caller, [`discard`](Self::discard), whose
    /// answer decides whether the rider is offered another attempt. [`finalize`](Self::finalize) and
    /// [`settle`](Self::settle) read the resulting state instead.
    async fn service_terminal(&mut self) -> Result<(), StoreError> {
        match self.state {
            State::FinaliseAfterRepair(mut live) => {
                // `RESUME` is the image staged by the failed ordinary checkpoint. The footer lives
                // just beyond `delta_len`, so this retry lends only the original sample delta.
                let resume = unsafe { *resume_slice() };
                match self.journal(live, &resume).await {
                    Ok(()) => {
                        let old_len = live.delta_len;
                        unsafe {
                            let delta = delta_mut();
                            delta.copy_within(old_len..old_len + FOOTER_LEN, 0);
                            delta[FOOTER_LEN..old_len + FOOTER_LEN].fill(0);
                        }
                        live.delta_len = 0;
                        live.journal_blocked = false;
                        let mut crc = live.crc;
                        crc.update(unsafe { delta_slice(FOOTER_LEN) });
                        self.state = State::Finalising(Finalising {
                            id: live.id,
                            revision: live.revision,
                            name: live.name,
                            delta_len: FOOTER_LEN,
                            payload_len: u64::from(live.points) * SAMPLE_LEN as u64 + FOOTER_LEN as u64,
                            payload_crc: crc.finalize(),
                            journaled: false,
                        });
                        Ok(())
                    }
                    Err(error) => {
                        self.warning_pending = true;
                        defmt::warn!("flat ride: pre-finish checkpoint repair failed: {}", defmt::Debug2Format(&error));
                        Err(error)
                    }
                }
            }
            State::Finalising(mut finalising) => {
                if !finalising.journaled {
                    let checkpoint = RideCheckpoint {
                        id: finalising.id,
                        revision: finalising.revision,
                        append: unsafe { delta_slice(finalising.delta_len) },
                        payload_crc: finalising.payload_crc,
                        resume: unsafe { resume_slice() },
                    };
                    match self.writer.call(Request::Journal { checkpoint }, &REPLY).await {
                        Ok(Outcome::Done) => {
                            finalising.journaled = true;
                            self.state = State::Finalising(finalising);
                        }
                        Ok(_) => return Err(StoreError::Invalid),
                        Err(error) => {
                            self.warning_pending = true;
                            defmt::warn!("flat ride: final checkpoint failed: {}", defmt::Debug2Format(&error));
                            return Err(error);
                        }
                    }
                }
                let meta = EntryMeta {
                    added_at_utc: 0,
                    id: finalising.id,
                    revision: finalising.revision,
                    kind: ObjectKind::Ride,
                    flags: EntryFlags::NONE,
                    payload_len: finalising.payload_len,
                    payload_crc: finalising.payload_crc,
                    name: finalising.name,
                };
                let mut batch = Vec::new();
                let _ = batch.push(Mutation::Put { meta, source: PutSource::Amend });
                match self.writer.call(Request::Commit { batch }, &REPLY).await {
                    Ok(Outcome::Committed(_)) => {
                        self.state = State::Idle;
                        defmt::info!("flat ride: finished {=u64} B with footer + one commit", finalising.payload_len);
                        Ok(())
                    }
                    Ok(_) => Err(StoreError::Invalid),
                    Err(error) => {
                        self.warning_pending = true;
                        defmt::warn!("flat ride: final commit failed: {}", defmt::Debug2Format(&error));
                        Err(error)
                    }
                }
            }
            State::Discarding { id, revision } => {
                let mut batch = Vec::new();
                let _ = batch.push(Mutation::Remove { id, revision });
                match self.writer.call(Request::Commit { batch }, &REPLY).await {
                    // The catalog no longer holding the entry is the goal state: an earlier attempt
                    // landed and only its answer was lost.
                    Ok(Outcome::Committed(_)) | Err(StoreError::NotFound) => {
                        self.state = State::Idle;
                        Ok(())
                    }
                    Ok(_) => Err(StoreError::Invalid),
                    // No warning is raised here: the caller reports the typed refusal, and for a
                    // damaged object the recovery card is the rider's one explanation.
                    Err(error) => Err(error),
                }
            }
            _ => Ok(()),
        }
    }
}

/// The one place a `Faulted` state is built, so every classified refusal announces itself the same
/// way: which object, which cause, and whether a repair may be offered for it.
fn faulted(id: ObjectId, revision: Revision, damage: RideDamage) -> State {
    let (name, repairable) = match damage {
        RideDamage::Payload => ("payload", true),
        RideDamage::Metadata => ("metadata", true),
        RideDamage::Catalog => ("catalog", false),
    };
    defmt::error!(
        "flat ride: damaged RECORDING {=u64} rev {=u64} -- {=str}, repairable={=bool}",
        id.0,
        revision.0,
        name,
        repairable
    );
    State::Faulted { id, revision, damage }
}

fn read_sample_time(store: &FlatStore<FlatCard>, offset: u64, total: u64) -> Option<u32> {
    if offset.checked_add(SAMPLE_LEN as u64)? > total {
        return None;
    }
    let mut sample = [0u8; SAMPLE_LEN];
    if store.read_recovered(offset, &mut sample).ok()? != SAMPLE_LEN {
        return None;
    }
    Some(obc_formats::track::decode_record(&sample).t_ms)
}

fn start_time(stats: &obc_route::RideStats, first_t_ms: Option<u32>) -> u32 {
    if !stats.clock_trusted {
        return 0;
    }
    let first = first_t_ms.unwrap_or(stats.anchor_ms);
    stats.unix_at_anchor.wrapping_sub(stats.anchor_ms.wrapping_sub(first) / 1000)
}

/// The ride loop is the only mutable owner. A journal request lends the storage task an immutable
/// view and waits for its reply before the loop can touch the buffer again. These raw-slice helpers
/// express that cross-task handoff without manufacturing a permanent borrow of a `static mut`.
unsafe fn delta_mut() -> &'static mut [u8; DELTA_BYTES] {
    &mut *addr_of_mut!(DELTA)
}

unsafe fn delta_slice(len: usize) -> &'static [u8] {
    core::slice::from_raw_parts(addr_of!(DELTA).cast::<u8>(), len)
}

unsafe fn resume_mut() -> &'static mut [u8; RIDE_RESUME_LEN] {
    &mut *addr_of_mut!(RESUME)
}

unsafe fn resume_slice() -> &'static [u8; RIDE_RESUME_LEN] {
    &*addr_of!(RESUME)
}

pub(crate) const RESIDENT_BYTES: usize = DELTA_BYTES + RIDE_RESUME_LEN;
