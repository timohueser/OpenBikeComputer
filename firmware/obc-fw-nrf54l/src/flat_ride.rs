//! Flat-store ride recording (FS8 #1390) — the board's half of the Recorder protocol (#1398).
//!
//! [`RecorderMachine`](obc_app::RecorderMachine) decides what a ride is and when it closes; this
//! module performs the physical operations that decision names: start one `RECORDING` object,
//! collect the final 20-byte sample bytes, checkpoint them through the tail journal, append one
//! final footer, and clear `RECORDING` in one commit. There is no temporary file and no finish-time
//! conversion, and no lifecycle rule here — one method per `RecorderEffect`, each answering whether
//! the store did it.

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

use crate::flat_store::{FlatCard, Outcome, Reply, Request, Writer};

const RIDE_RESERVE: u64 = 32 * 1024 * 1024;
// At the minimum 1 s fix cadence, one checkpoint interval contributes at most ten records. Six
// extra records cover a delayed pass, and the fixed footer always keeps its own reserved space.
// The store owns the durable partial 16 KiB page; the board retains only bytes appended since the
// last successful logical checkpoint.
const DELTA_SAMPLES: usize = 16;
const DELTA_BYTES: usize = DELTA_SAMPLES * SAMPLE_LEN + FOOTER_LEN;

static REPLY: Reply = Signal::<CriticalSectionRawMutex, _>::new();
static mut DELTA: [u8; DELTA_BYTES] = [0; DELTA_BYTES];
static mut RESUME: [u8; RIDE_RESUME_LEN] = [0; RIDE_RESUME_LEN];

const RESUME_MAGIC: [u8; 4] = *b"OBRC";
const RESUME_VERSION: u16 = 1;

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
    /// Stable UTC start persisted with every logical checkpoint. A same-boot short ride can derive
    /// it at Finish; a continued ride must never combine an old monotonic sample with a new boot's
    /// wall-clock anchor.
    start_time: Option<u32>,
    /// Only a ride that is still in the boot where its first sample was recorded may derive a
    /// trusted UTC start later. Once recovered, `first_t_ms` belongs to the old monotonic domain;
    /// combining it with this boot's wall anchor would back-date the ride by nearly 2^32 ms.
    can_upgrade_start: bool,
    clock_rebase: Option<ClockRebase>,
    continuation: obc_app::RideContinuation,
    /// A failed checkpoint blocks further samples until the exact same append has been retried. This
    /// preserves storage's already-gated rollover recovery anchor and bounds loss under repeated
    /// media faults instead of letting the caller mutate the retry payload.
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
    /// `live.delta_len` but is not part of that length or CRC yet: storage must first see the exact
    /// failed append + resume again. Once repair succeeds the footer moves to offset zero and the
    /// normal final checkpoint publishes it.
    FinaliseAfterRepair(Live),
    /// A durable `RECORDING` object whose bytes are not a provable ride-v3 boundary. It remains
    /// visible to recovery diagnostics but is never attached to a session or appended to.
    Faulted {
        id: ObjectId,
        revision: Revision,
    },
    Finalising(Finalising),
    Discarding {
        id: ObjectId,
        revision: Revision,
    },
}

pub(crate) struct Recorder {
    state: State,
    writer: Writer,
    warning_pending: bool,
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
                        state: State::Faulted { id: recovered.id, revision: recovered.revision },
                        writer,
                        warning_pending: true,
                    };
                }
                let catalog_name = catalog_name.unwrap_or_default();

                // A footer-bearing checkpoint means Finish made the final bytes durable and only
                // the clearing commit was cut. Validate the footer before completing it; otherwise
                // the payload remains a resumable sequence of exact 20-byte samples.
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
                                };
                            }
                        }
                    }
                }

                let sample_anchors_valid = total == 0 || (first_t_ms.is_some() && last_t_ms.is_some());
                if total.is_multiple_of(SAMPLE_LEN as u64) && sample_anchors_valid {
                    let resumed = if total == 0 {
                        Some((obc_app::RideContinuation::default(), None))
                    } else {
                        decode_resume(&recovered.resume)
                    };
                    let Some((continuation, start_time)) = resumed else {
                        defmt::error!("flat ride: recovered samples have no valid continuation metadata");
                        return Self {
                            state: State::Faulted { id: recovered.id, revision: recovered.revision },
                            writer,
                            warning_pending: true,
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
                    // The store proved a durable checkpoint, but it is not a ride-v3 sample/footer
                    // boundary. Keep the RECORDING object intact and loud; never append to or
                    // publish bytes whose domain format this executor cannot prove.
                    defmt::error!("flat ride: recovered payload is not a v3 sample/footer boundary");
                    State::Faulted { id: recovered.id, revision: recovered.revision }
                }
            }
        };
        Recorder { state, writer, warning_pending: false }
    }

    /// Whether a durable `RECORDING` object is open, closing, or faulted — the DFU arm's refusal
    /// check.
    pub(crate) fn is_recording(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    /// The ride session the **live** object belongs to, or `None` when no object is attached to
    /// one. A faulted or closing object answers `None`: neither is a session's to record into.
    pub(crate) fn open_session(&self) -> Option<u32> {
        match self.state {
            State::Live(live) => live.session,
            _ => None,
        }
    }

    /// The app-side state paired with a recovered logical checkpoint. The board restores this
    /// before showing the explicit Continue/Discard card; malformed metadata never reaches the UI.
    pub(crate) fn recovered_continuation(&self) -> Option<obc_app::RideContinuation> {
        let State::Live(Live { session: None, continuation, .. }) = self.state else { return None };
        Some(continuation)
    }

    pub(crate) fn recovery_faulted(&self) -> bool {
        matches!(self.state, State::Faulted { .. })
    }

    pub(crate) fn take_warning(&mut self) -> bool {
        core::mem::take(&mut self.warning_pending)
    }

    /// Service a terminal state left over from a reset — a footer-bearing recovered object whose
    /// clearing commit was cut. Run once at boot, before the first UI pass.
    pub(crate) async fn settle(&mut self) {
        self.service_terminal().await;
    }

    /// Open a ride object for `session`, saved as `name` — Recorder's session edge.
    ///
    /// A recovered object with no session attached adopts this one instead of starting a second:
    /// that is what "continue" means, and it is the only way the restored samples keep their object.
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

    /// Close the ride into a durable ride object.
    ///
    /// [`RideClose::Failed`](obc_app::recorder::RideClose) leaves the object `RECORDING` on the
    /// card with its staged footer staged, and Recorder re-offers the same finalize — which
    /// re-enters the terminal service below at exactly the step that failed. `Nothing` is the
    /// honest answer when no object was ever created: a start this card refused already warned the
    /// rider, and reporting a failure would retry a close against something that does not exist for
    /// the rest of the boot.
    ///
    /// The save name is **not** a parameter: it was frozen when the ride opened
    /// ([`open`](Self::open)), so a mid-ride route swap cannot rename a ride that is already
    /// recording.
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
        self.service_terminal().await;
        match self.state {
            State::Idle => RideClose::Committed(closing),
            _ => RideClose::Failed,
        }
    }

    /// Delete the open ride and its journal. `false` is a failure and Recorder re-offers it.
    ///
    /// Deletion deliberately does not repair a blocked checkpoint. `Remove` never enters storage's
    /// final-tail flush: the atomic catalog mutation first makes the ride/proof unreachable, then
    /// `settle_ride` clears its pending recovery state and invalidates the journal headers. Repair
    /// would add fallible I/O to an object the rider explicitly asked to destroy.
    pub(crate) async fn discard(&mut self) -> bool {
        match self.state {
            State::Live(live) | State::FinaliseAfterRepair(live) => {
                self.state = State::Discarding { id: live.id, revision: live.revision };
            }
            State::Faulted { id, revision } => self.state = State::Discarding { id, revision },
            _ => {}
        }
        self.service_terminal().await;
        matches!(self.state, State::Idle)
    }

    /// Append one staged sample to the bounded tail. `false` is a refusal, and Recorder keeps that
    /// sample and everything behind it staged rather than losing them.
    ///
    /// A refusal is always transient and always cleared by the checkpoint Recorder ranks ahead of
    /// the append: either the journal is blocked (storage needs the exact failed write replayed
    /// before it takes anything else), or the delta window is full (a checkpoint empties it). The
    /// fixed footer's space stays reserved through both, because accepting a sample that displaced
    /// it would strand the durable `RECORDING` object after the ride was already closed.
    pub(crate) fn append(&mut self, mut point: TrackPoint) -> bool {
        let State::Live(mut live) = self.state else { return false };
        if live.session.is_none() || live.journal_blocked || live.delta_len + SAMPLE_LEN + FOOTER_LEN > DELTA_BYTES {
            return false;
        }
        if let Some(clock) = live.clock_rebase {
            point.t_ms = clock.logical_anchor.wrapping_add(point.t_ms.wrapping_sub(clock.source_anchor));
        }
        let Some(points) = live.points.checked_add(1) else { return false };
        let sample = obc_formats::track::encode_record(&point);
        unsafe { delta_mut()[live.delta_len..live.delta_len + SAMPLE_LEN].copy_from_slice(&sample) };
        live.delta_len += SAMPLE_LEN;
        live.points = points;
        live.first_t_ms.get_or_insert(point.t_ms);
        live.last_t_ms = Some(point.t_ms);
        live.crc.update(&sample);
        self.state = State::Live(live);
        true
    }

    /// Make the ride recoverable up to this point. `false` is a failed journal write; Recorder owes
    /// the same checkpoint again, and storage's equality contract needs it to be exactly the same.
    pub(crate) async fn checkpoint(
        &mut self,
        now_ms: u32,
        stats: &obc_route::RideStats,
        continuation: obc_app::RideContinuation,
    ) -> bool {
        let State::Live(live) = self.state else { return true };
        // Once an attempt fails, storage's equality contract requires the *entire* logical
        // checkpoint to be replayed: append, CRC and opaque resume. App totals can keep moving even
        // while samples are frozen, so never rebuild resume from the current app on a retry.
        let (resume, attempted_continuation, attempted_start) = if live.journal_blocked {
            (unsafe { *resume_slice() }, live.continuation, live.start_time)
        } else {
            let stable_start = live.start_time.or_else(|| {
                (live.can_upgrade_start && stats.clock_trusted).then(|| start_time(stats, live.first_t_ms))
            });
            (encode_resume(continuation, stable_start), continuation, stable_start)
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
                true
            }
            Err(error) => {
                let mut blocked = live;
                // These now name the staged resume, not the last durable one. Keeping them beside
                // `journal_blocked` makes a later successful retry advance the in-RAM state to the
                // exact snapshot storage just accepted rather than to newer app totals.
                blocked.start_time = attempted_start;
                blocked.continuation = attempted_continuation;
                blocked.journal_blocked = true;
                self.state = State::Live(blocked);
                self.warning_pending = true;
                defmt::warn!("flat ride: checkpoint failed: {}", defmt::Debug2Format(&error));
                false
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
        // first-sample timestamp with this boot's wall anchor and therefore reports start 0.
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

    async fn service_terminal(&mut self) {
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
                    }
                    Err(error) => {
                        self.warning_pending = true;
                        defmt::warn!("flat ride: pre-finish checkpoint repair failed: {}", defmt::Debug2Format(&error));
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
                        Ok(_) => return,
                        Err(error) => {
                            self.warning_pending = true;
                            defmt::warn!("flat ride: final checkpoint failed: {}", defmt::Debug2Format(&error));
                            return;
                        }
                    }
                }
                let meta = EntryMeta {
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
                    }
                    Ok(_) => {}
                    Err(error) => {
                        self.warning_pending = true;
                        defmt::warn!("flat ride: final commit failed: {}", defmt::Debug2Format(&error));
                    }
                }
            }
            State::Discarding { id, revision } => {
                let mut batch = Vec::new();
                let _ = batch.push(Mutation::Remove { id, revision });
                match self.writer.call(Request::Commit { batch }, &REPLY).await {
                    Ok(Outcome::Committed(_)) => self.state = State::Idle,
                    Ok(_) => {}
                    Err(error) => {
                        self.warning_pending = true;
                        defmt::warn!("flat ride: discard failed: {}", defmt::Debug2Format(&error));
                    }
                }
            }
            _ => {}
        }
    }
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

fn encode_resume(state: obc_app::RideContinuation, start_time: Option<u32>) -> [u8; RIDE_RESUME_LEN] {
    const _: () = assert!(RIDE_RESUME_LEN == 96);
    let mut out = [0u8; RIDE_RESUME_LEN];
    out[0..4].copy_from_slice(&RESUME_MAGIC);
    out[4..6].copy_from_slice(&RESUME_VERSION.to_le_bytes());
    out[6..8].copy_from_slice(&(RIDE_RESUME_LEN as u16).to_le_bytes());
    out[8..12].copy_from_slice(&start_time.unwrap_or(0).to_le_bytes());
    for (at, value) in
        [(12, state.ridden_m), (16, state.moving_m), (20, state.moving_s), (24, state.climb_m), (28, state.descent_m)]
    {
        out[at..at + 4].copy_from_slice(&value.to_bits().to_le_bytes());
    }
    out[32..40].copy_from_slice(&state.hr_ms_sum.to_le_bytes());
    out[40..44].copy_from_slice(&state.hr_ms.to_le_bytes());
    out[44..46].copy_from_slice(&state.max_hr.to_le_bytes());
    out[48..56].copy_from_slice(&state.power_ms_sum.to_le_bytes());
    out[56..60].copy_from_slice(&state.power_ms.to_le_bytes());
    out[60..62].copy_from_slice(&state.max_power.to_le_bytes());
    out[64..72].copy_from_slice(&state.cadence_ms_sum.to_le_bytes());
    out[72..76].copy_from_slice(&state.cadence_ms.to_le_bytes());
    out[76] = u8::from(start_time.is_some());
    out
}

fn decode_resume(bytes: &[u8; RIDE_RESUME_LEN]) -> Option<(obc_app::RideContinuation, Option<u32>)> {
    if bytes[0..4] != RESUME_MAGIC
        || u16::from_le_bytes(bytes[4..6].try_into().ok()?) != RESUME_VERSION
        || u16::from_le_bytes(bytes[6..8].try_into().ok()?) as usize != RIDE_RESUME_LEN
        || bytes[46..48].iter().any(|byte| *byte != 0)
        || bytes[62..64].iter().any(|byte| *byte != 0)
        || bytes[76] > 1
        || bytes[77..].iter().any(|byte| *byte != 0)
    {
        return None;
    }
    let f32_at = |at: usize| {
        let mut raw = [0u8; 4];
        raw.copy_from_slice(&bytes[at..at + 4]);
        f32::from_bits(u32::from_le_bytes(raw))
    };
    let state = obc_app::RideContinuation {
        ridden_m: f32_at(12),
        moving_m: f32_at(16),
        moving_s: f32_at(20),
        climb_m: f32_at(24),
        descent_m: f32_at(28),
        hr_ms_sum: u64::from_le_bytes(bytes[32..40].try_into().ok()?),
        hr_ms: u32::from_le_bytes(bytes[40..44].try_into().ok()?),
        max_hr: u16::from_le_bytes(bytes[44..46].try_into().ok()?),
        power_ms_sum: u64::from_le_bytes(bytes[48..56].try_into().ok()?),
        power_ms: u32::from_le_bytes(bytes[56..60].try_into().ok()?),
        max_power: u16::from_le_bytes(bytes[60..62].try_into().ok()?),
        cadence_ms_sum: u64::from_le_bytes(bytes[64..72].try_into().ok()?),
        cadence_ms: u32::from_le_bytes(bytes[72..76].try_into().ok()?),
    };
    let finite_nonnegative = [state.ridden_m, state.moving_m, state.moving_s, state.climb_m, state.descent_m]
        .iter()
        .all(|value| value.is_finite() && *value >= 0.0);
    let start = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let start = (bytes[76] == 1).then_some(start);
    finite_nonnegative.then_some((state, start))
}

/// The ride loop is the only mutable owner. A journal request lends the storage task an immutable
/// view and waits for its reply before the loop can touch the buffer again. These raw-slice helpers
/// express that cross-task handoff without manufacturing a permanent Rust borrow of a `static mut`.
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
