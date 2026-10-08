//! Board storage transport for the shared durable ride writer.

use crate::flat_store::{FlatCard, Outcome, Reply, Request, Writer};
use core::ptr::{addr_of, addr_of_mut};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use heapless::Vec;
pub(crate) use obc_app::recorder::writer::AppendResult;
use obc_app::recorder::writer::{Recovery, RideWriter, Write, WriteBuffers, RESERVE_BYTES};
use obc_app::recorder::{CheckpointStatus, RecorderEffect, RecorderError, RecorderOutcome, RideClose};
use obc_app::RideDamage;
use obc_ports::TrackPoint;
use obc_storage::flat::{
    DisplayName, EntryFlags, EntryMeta, FlatStore, Mutation, ObjectId, ObjectKind, PutSource, Revision, RideCheckpoint,
    Store as _, StoreError, RIDE_RESUME_LEN,
};

const _: () = assert!(obc_app::recorder::CHECKPOINT_RETRY_MS == obc_storage::health::Breaker::COOL_DOWN_MS);
const _: () = assert!(RIDE_RESUME_LEN == obc_app::recorder::continuation::RIDE_RESUME_LEN);
static REPLY: Reply = Signal::<CriticalSectionRawMutex, _>::new();
static mut BUFFERS: WriteBuffers = WriteBuffers::ZERO;

#[derive(Clone, Copy)]
struct Key {
    id: ObjectId,
    revision: Revision,
}

pub(crate) struct Recorder {
    ride: RideWriter,
    key: Option<Key>,
    writer: Writer,
    warning_pending: bool,
    #[cfg(feature = "debug-uart")]
    repair_fail_once: bool,
}

impl Recorder {
    pub(crate) fn new(store: &'static FlatStore<FlatCard>, writer: Writer, _now_ms: u32) -> Self {
        let mut recorder = Self {
            ride: RideWriter::idle(),
            key: None,
            writer,
            warning_pending: false,
            #[cfg(feature = "debug-uart")]
            repair_fail_once: false,
        };
        if let Some(recovered) = store.recovered_ride() {
            recorder.key = Some(Key { id: recovered.id, revision: recovered.revision });
            let entry = store.find_entry(|entry| (entry.id, entry.revision) == (recovered.id, recovered.revision));
            recorder.ride = match entry {
                Ok(Some(entry)) => RideWriter::recover(
                    entry.name.as_str().unwrap_or(""),
                    Recovery {
                        payload_len: recovered.payload_len(),
                        payload_crc: recovered.payload_crc,
                        checkpoint_sequence: recovered.checkpoint_sequence,
                        resume: &recovered.resume,
                    },
                    |at, bytes| store.read_recovered(at, bytes),
                )
                .unwrap_or_else(|_| RideWriter::damaged(RideDamage::Catalog)),
                _ => RideWriter::damaged(RideDamage::Catalog),
            };
            if let Some(damage) = recorder.ride.recovery_damage() {
                defmt::error!("flat ride: damaged recovery: {}", defmt::Debug2Format(&damage));
            }
        }
        recorder
    }

    pub(crate) fn is_recording(&self) -> bool {
        !self.ride.is_idle()
    }
    pub(crate) fn open_session(&self) -> Option<u32> {
        self.ride.session()
    }
    pub(crate) fn recovered_continuation(&self) -> Option<obc_app::RideContinuation> {
        self.ride.recovered_continuation()
    }
    pub(crate) fn recovery_damage(&self) -> Option<RideDamage> {
        self.ride.recovery_damage()
    }
    pub(crate) fn take_warning(&mut self) -> bool {
        core::mem::take(&mut self.warning_pending)
    }

    pub(crate) async fn settle(&mut self) {
        if self.ride.is_closing() && self.flush().await.is_err() {
            self.warning_pending = true;
        }
    }

    pub(crate) async fn open(&mut self, store: &'static FlatStore<FlatCard>, session: u32, name: &str, now_ms: u32) {
        if self.ride.is_idle() {
            if let Err(error) = self.start(store, Some(session), name).await {
                self.warning_pending = true;
                defmt::warn!("flat ride: start failed: {}", defmt::Debug2Format(&error));
            }
        } else {
            self.ride.attach(session, name, now_ms);
        }
    }
    pub(crate) async fn execute(
        &mut self,
        store: &'static FlatStore<FlatCard>,
        app: &obc_app::App,
        opened_session: &mut Option<u32>,
        effect: Option<RecorderEffect>,
        now: u32,
    ) -> Option<RecorderOutcome> {
        if let Some(id) = app.recorder.object_owed(*opened_session) {
            let name = app.ride_name().unwrap_or("");
            self.open(store, id, name, now).await;
            // A refused start stays owed and is retried on the next iteration.
            if self.open_session() == Some(id) {
                *opened_session = Some(id);
            }
        }
        Some(match effect? {
            RecorderEffect::Checkpoint { token } => {
                let stats = app.ride_stats();
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
                let stats = app.ride_stats();
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

    pub(crate) async fn finalize(&mut self, stats: &obc_route::RideStats) -> RideClose {
        if self.ride.is_idle() {
            return RideClose::Nothing;
        }
        self.ride.finish(unsafe { buffers_mut() }, stats);
        if !self.ride.is_closing() {
            return RideClose::Failed;
        }
        let Some(key) = self.key else { return RideClose::Failed };
        match self.flush().await {
            Ok(()) => RideClose::Committed(key.id.0),
            Err(_) => {
                self.warning_pending = true;
                RideClose::Failed
            }
        }
    }

    pub(crate) async fn discard(&mut self) -> Result<(), StoreError> {
        self.ride.discard();
        #[cfg(feature = "debug-uart")]
        if core::mem::take(&mut self.repair_fail_once) && self.ride.pending_write() == Some(Write::Remove) {
            return Err(StoreError::Media);
        }
        self.flush().await
    }

    pub(crate) fn append(&mut self, points: &[TrackPoint], continuation: obc_app::RideContinuation) -> AppendResult {
        self.ride.append(unsafe { buffers_mut() }, points, continuation)
    }

    pub(crate) async fn checkpoint(
        &mut self,
        _now_ms: u32,
        stats: &obc_route::RideStats,
        continuation: Option<obc_app::RideContinuation>,
    ) -> Result<CheckpointStatus, RecorderError> {
        if !self.ride.checkpoint(unsafe { buffers_mut() }, stats, continuation) {
            return Err(RecorderError::Write);
        }
        match self.flush().await {
            Ok(()) => Ok(CheckpointStatus::Durable),
            Err(error) => {
                self.warning_pending = true;
                defmt::warn!("flat ride: checkpoint failed: {}", defmt::Debug2Format(&error));
                Err(RecorderError::Write)
            }
        }
    }

    async fn start(
        &mut self,
        store: &'static FlatStore<FlatCard>,
        session: Option<u32>,
        name: &str,
    ) -> Result<(), StoreError> {
        let allocation = match self.writer.call(Request::Allocate { bytes: RESERVE_BYTES }, &REPLY).await? {
            Outcome::Allocated(allocation) => allocation,
            _ => return Err(StoreError::Invalid),
        };
        let key = Key { id: store.next_object_id(), revision: Revision(1) };
        let name = DisplayName::new(name).unwrap_or_default();
        let meta = EntryMeta {
            added_at_utc: 0,
            id: key.id,
            revision: key.revision,
            kind: ObjectKind::Ride,
            flags: EntryFlags::RECORDING,
            payload_len: 0,
            payload_crc: 0,
            name,
        };
        match self.commit(Mutation::Put { meta, source: PutSource::Fresh(allocation) }).await {
            Ok(()) => {
                self.key = Some(key);
                self.ride = RideWriter::start(name.as_str().unwrap_or(""), session);
                Ok(())
            }
            Err(error) => {
                let _ = self.writer.call(Request::Cancel { allocation }, &REPLY).await;
                Err(error)
            }
        }
    }

    async fn commit(&self, mutation: Mutation) -> Result<(), StoreError> {
        let mut batch = Vec::new();
        batch.push(mutation).map_err(|_| StoreError::Invalid)?;
        match self.writer.call(Request::Commit { batch }, &REPLY).await? {
            Outcome::Committed(_) => Ok(()),
            _ => Err(StoreError::Invalid),
        }
    }

    async fn flush(&mut self) -> Result<(), StoreError> {
        while let Some(write) = self.ride.pending_write() {
            let key = self.key.ok_or(StoreError::Invalid)?;
            match write {
                Write::Journal { append_len, payload_crc } => {
                    let buffers = unsafe { buffers() };
                    let checkpoint = RideCheckpoint {
                        id: key.id,
                        revision: key.revision,
                        append: &buffers.append[..append_len],
                        payload_crc,
                        resume: &buffers.resume,
                    };
                    match self.writer.call(Request::Journal { checkpoint }, &REPLY).await? {
                        Outcome::Done => {}
                        _ => return Err(StoreError::Invalid),
                    }
                }
                Write::Publish { name, payload_len, payload_crc } => {
                    let meta = EntryMeta {
                        added_at_utc: 0,
                        id: key.id,
                        revision: key.revision,
                        kind: ObjectKind::Ride,
                        flags: EntryFlags::NONE,
                        payload_len,
                        payload_crc,
                        name: DisplayName::new(name.as_str()).unwrap_or_default(),
                    };
                    self.commit(Mutation::Put { meta, source: PutSource::Amend }).await?;
                }
                Write::Remove => match self.commit(Mutation::Remove { id: key.id, revision: key.revision }).await {
                    Ok(()) | Err(StoreError::NotFound) => {}
                    Err(error) => return Err(error),
                },
            }
            self.ride.acknowledge(unsafe { buffers_mut() });
            if self.ride.is_idle() {
                self.key = None;
            }
        }
        Ok(())
    }

    #[cfg(feature = "debug-uart")]
    pub(crate) async fn debug_fabricate_damage(
        &mut self,
        store: &'static FlatStore<FlatCard>,
        kind: obc_platform::debug_link::RideDamageKind,
        _now_ms: u32,
    ) {
        use obc_platform::debug_link::RideDamageKind;
        if !self.ride.is_idle() {
            return;
        }
        if let Err(error) = self.start(store, None, "DAMAGED").await {
            defmt::error!("flat ride: damage setup failed: {}", defmt::Debug2Format(&error));
            return;
        }
        let Some(key) = self.key else { return };
        let (len, resume, damage) = match kind {
            RideDamageKind::Payload => (
                7,
                obc_app::recorder::continuation::encode(obc_app::RideContinuation::default(), None),
                RideDamage::Payload,
            ),
            RideDamageKind::Metadata => (obc_formats::ride::SAMPLE_LEN, [0; RIDE_RESUME_LEN], RideDamage::Metadata),
        };
        unsafe {
            buffers_mut().append[..len].fill(0);
            buffers_mut().resume = resume;
        }
        let bytes = unsafe { buffers() };
        let mut crc = obc_crc::Crc32::new();
        crc.update(&bytes.append[..len]);
        let checkpoint = RideCheckpoint {
            id: key.id,
            revision: key.revision,
            append: &bytes.append[..len],
            payload_crc: crc.finalize(),
            resume: &bytes.resume,
        };
        match self.writer.call(Request::Journal { checkpoint }, &REPLY).await {
            Ok(Outcome::Done) => self.ride = RideWriter::damaged(damage),
            Err(error) => {
                defmt::error!("flat ride: damage journal failed: {}", defmt::Debug2Format(&error));
            }
            Ok(_) => defmt::error!("flat ride: invalid damage journal reply"),
        }
    }

    #[cfg(feature = "debug-uart")]
    pub(crate) fn debug_arm_repair_failure(&mut self) {
        self.repair_fail_once = true;
    }
}

// The ride loop is the only mutable owner. The writer task borrows the bytes until its reply;
// no buffer mutation or acknowledgement is allowed while that request is outstanding.
unsafe fn buffers_mut() -> &'static mut WriteBuffers {
    &mut *addr_of_mut!(BUFFERS)
}
unsafe fn buffers() -> &'static WriteBuffers {
    &*addr_of!(BUFFERS)
}

pub(crate) const RESIDENT_BYTES: usize = core::mem::size_of::<WriteBuffers>();
