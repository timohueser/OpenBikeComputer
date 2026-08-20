//! Flat-store ride recording (FS8 #1390).
//!
//! The app still owns the pre-Recorder UI/session intent; this module owns only the primitive
//! board execution boundary FS8 promises to the later Recorder cutover: start one `RECORDING`
//! object, collect the final 20-byte sample bytes, checkpoint them through the tail journal,
//! append one final footer, and clear `RECORDING` in one commit. There is no temporary file and no
//! finish-time conversion.

use core::ptr::{addr_of, addr_of_mut};

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use heapless::Vec;
use obc_crc::Crc32;
use obc_formats::ride::{decode_footer, FOOTER_LEN, SAMPLE_LEN};
use obc_ports::{TrackError, TrackPoint, TrackSink};
use obc_storage::flat::{
    DisplayName, EntryFlags, EntryMeta, FlatStore, Mutation, ObjectId, ObjectKind, PutSource, Revision, RideCheckpoint,
    Store as _, StoreError, RIDE_RESUME_LEN,
};

use crate::flat_store::{FlatCard, Outcome, Reply, Request, Writer};

const CHECKPOINT_MS: u32 = 10_000;
const PAYLOAD_PAGE: usize = 16 * 1024;
const RIDE_RESERVE: u64 = 32 * 1024 * 1024;
// At the minimum 1 s fix cadence, one checkpoint interval contributes at most ten records. Six
// extra records cover a delayed pass without spending the full journal slot twice in RAM.
const TAIL_BYTES: usize = PAYLOAD_PAGE + 16 * SAMPLE_LEN;

static REPLY: Reply = Signal::<CriticalSectionRawMutex, _>::new();
static mut TAIL: [u8; TAIL_BYTES] = [0; TAIL_BYTES];
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
    tail_len: usize,
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
    /// A failed checkpoint blocks further samples until the exact same tail has been retried. This
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
    tail_len: usize,
    payload_len: u64,
    payload_crc: u32,
    journaled: bool,
}

#[derive(Clone, Copy)]
enum State {
    Idle,
    Live(Live),
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
                let tail_len = match store.recovered_tail(unsafe { tail_mut() }) {
                    Ok(len) => len,
                    Err(error) => {
                        defmt::error!("flat ride: recovered tail read failed: {}", defmt::Debug2Format(&error));
                        return Self {
                            state: State::Faulted { id: recovered.id, revision: recovered.revision },
                            writer,
                            warning_pending: true,
                        };
                    }
                };
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
                                        tail_len,
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
                if total.is_multiple_of(SAMPLE_LEN as u64)
                    && tail_len == recovered.tail_len as usize
                    && sample_anchors_valid
                {
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
                        tail_len,
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

    pub(crate) fn is_recording(&self) -> bool {
        !matches!(self.state, State::Idle)
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

    /// Whether the ride loop owes this executor work even when no app identity edge moved. This is
    /// what makes transient start, checkpoint-at-finish, final-commit, and discard failures retry
    /// on a later pass instead of being stranded behind `prev_session`.
    pub(crate) fn needs_reconcile(&self, session: Option<u32>) -> bool {
        match self.state {
            State::Idle => session.is_some(),
            State::Live(live) => live.session.is_none() && session.is_some(),
            State::Finalising(_) | State::Discarding { .. } => true,
            State::Faulted { .. } => false,
        }
    }

    pub(crate) fn track_sink(&mut self, session: Option<u32>) -> Option<&mut dyn TrackSink> {
        // While a ride session exists, absence of a writable recorder is itself a recording error.
        // Returning this failing sink makes App surface REC_ERROR instead of silently dropping GPS
        // fixes until a failed allocation/start happens to recover.
        session.map(|_| self as &mut dyn TrackSink)
    }

    pub(crate) fn checkpoint_is_due(&self, now_ms: u32) -> bool {
        matches!(
            self.state,
            State::Live(Live { session: Some(_), last_checkpoint_ms, journal_blocked, .. })
                if journal_blocked || now_ms.wrapping_sub(last_checkpoint_ms) >= CHECKPOINT_MS
        )
    }

    pub(crate) async fn reconcile(
        &mut self,
        store: &'static FlatStore<FlatCard>,
        action: Option<obc_app::TrackAction>,
        session: Option<u32>,
        name: &str,
        stats: Option<&obc_route::RideStats>,
        now_ms: u32,
    ) {
        match action {
            Some(obc_app::TrackAction::Save) => {
                if let (State::Live(live), Some(stats)) = (self.state, stats) {
                    self.begin_finalise(live, name, stats);
                }
            }
            Some(obc_app::TrackAction::Discard) => match self.state {
                State::Live(live) => {
                    self.state = State::Discarding { id: live.id, revision: live.revision };
                }
                State::Faulted { id, revision } => {
                    self.state = State::Discarding { id, revision };
                }
                _ => {}
            },
            None => {}
        }

        match self.state {
            State::Idle if session.is_some() => {
                if let Err(error) = self.start(store, session, name, now_ms).await {
                    self.warning_pending = true;
                    defmt::warn!("flat ride: start failed: {}", defmt::Debug2Format(&error));
                }
            }
            State::Live(mut live) if live.session.is_none() && session.is_some() => {
                live.session = session;
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

        self.service_terminal().await;
    }

    pub(crate) async fn checkpoint_due(
        &mut self,
        now_ms: u32,
        stats: &obc_route::RideStats,
        continuation: obc_app::RideContinuation,
    ) {
        let State::Live(live) = self.state else { return };
        if !self.checkpoint_is_due(now_ms) {
            return;
        }
        let stable_start = live
            .start_time
            .or_else(|| (live.can_upgrade_start && stats.clock_trusted).then(|| start_time(stats, live.first_t_ms)));
        let resume = encode_resume(continuation, stable_start);
        match self.journal(live, &resume).await {
            Ok(()) => {
                let mut next = live;
                compact_tail(&mut next.tail_len);
                next.last_checkpoint_ms = now_ms;
                next.start_time = stable_start;
                next.continuation = continuation;
                next.journal_blocked = false;
                self.state = State::Live(next);
            }
            Err(error) => {
                let mut blocked = live;
                blocked.journal_blocked = true;
                self.state = State::Live(blocked);
                self.warning_pending = true;
                defmt::warn!("flat ride: checkpoint failed: {}", defmt::Debug2Format(&error));
            }
        }
    }

    fn begin_finalise(&mut self, live: Live, fallback_name: &str, stats: &obc_route::RideStats) {
        let name = if live.name.is_empty() { DisplayName::new(fallback_name).unwrap_or_default() } else { live.name };
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
        if live.tail_len + footer.len() > TAIL_BYTES {
            defmt::error!("flat ride: footer does not fit the bounded tail");
            return;
        }
        unsafe { tail_mut()[live.tail_len..live.tail_len + footer.len()].copy_from_slice(&footer) };
        let mut crc = live.crc;
        crc.update(&footer);
        self.state = State::Finalising(Finalising {
            id: live.id,
            revision: live.revision,
            name,
            tail_len: live.tail_len + footer.len(),
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
                unsafe { tail_mut().fill(0) };
                self.state = State::Live(Live {
                    session,
                    id,
                    revision,
                    name,
                    tail_len: 0,
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
            tail: unsafe { tail_slice(live.tail_len) },
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
            State::Finalising(mut finalising) => {
                if !finalising.journaled {
                    let checkpoint = RideCheckpoint {
                        id: finalising.id,
                        revision: finalising.revision,
                        tail: unsafe { tail_slice(finalising.tail_len) },
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

impl TrackSink for Recorder {
    fn record(&mut self, mut point: TrackPoint) -> Result<(), TrackError> {
        let State::Live(mut live) = self.state else { return Err(TrackError) };
        // Keep the fixed footer's space reserved even through repeated checkpoint failures. Finish
        // is a one-shot UI edge; accepting a sample that displaced the footer would strand the
        // durable RECORDING object after the app had already ended its session.
        if live.session.is_none() || live.journal_blocked || live.tail_len + SAMPLE_LEN + FOOTER_LEN > TAIL_BYTES {
            return Err(TrackError);
        }
        if let Some(clock) = live.clock_rebase {
            point.t_ms = clock.logical_anchor.wrapping_add(point.t_ms.wrapping_sub(clock.source_anchor));
        }
        let sample = obc_formats::track::encode_record(&point);
        unsafe { tail_mut()[live.tail_len..live.tail_len + SAMPLE_LEN].copy_from_slice(&sample) };
        live.tail_len += SAMPLE_LEN;
        live.points = live.points.checked_add(1).ok_or(TrackError)?;
        live.first_t_ms.get_or_insert(point.t_ms);
        live.last_t_ms = Some(point.t_ms);
        live.crc.update(&sample);
        self.state = State::Live(live);
        Ok(())
    }
}

fn compact_tail(len: &mut usize) {
    let flushed = *len / PAYLOAD_PAGE * PAYLOAD_PAGE;
    if flushed == 0 {
        return;
    }
    let remain = *len - flushed;
    unsafe {
        let tail = tail_mut();
        tail.copy_within(flushed..*len, 0);
        tail[remain..*len].fill(0);
    }
    *len = remain;
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
unsafe fn tail_mut() -> &'static mut [u8; TAIL_BYTES] {
    &mut *addr_of_mut!(TAIL)
}

unsafe fn tail_slice(len: usize) -> &'static [u8] {
    core::slice::from_raw_parts(addr_of!(TAIL).cast::<u8>(), len)
}

unsafe fn resume_mut() -> &'static mut [u8; RIDE_RESUME_LEN] {
    &mut *addr_of_mut!(RESUME)
}

unsafe fn resume_slice() -> &'static [u8; RIDE_RESUME_LEN] {
    &*addr_of!(RESUME)
}

pub(crate) const RESIDENT_BYTES: usize = TAIL_BYTES + RIDE_RESUME_LEN;
