//! Host storage transport for the shared durable ride writer.

use crate::flat_store::{HostStore, MountedStore};
use crate::repo::{AppendStatus, TrackRepository};
use obc_app::recorder::writer::{AppendResult, Recovery, RideWriter, Write, WriteBuffers, RESERVE_BYTES};
use obc_app::recorder::{CheckpointStatus, RecorderError, RideClose, RideContinuation};
use obc_app::{App, RideDamage};
use obc_ports::TrackPoint;
use obc_route::RideStats;
use obc_storage::flat::{
    DisplayName, EntryFlags, EntryMeta, Mutation, ObjectId, ObjectKind, PutSource, Revision, RideCheckpoint, Store,
    StoreError, StoreId,
};

#[derive(Clone, Copy)]
struct Key {
    store: StoreId,
    id: ObjectId,
    revision: Revision,
}

impl Key {
    fn check(self, owner: &MountedStore) -> Result<EntryMeta, StoreError> {
        let card = owner.ready().map_err(|_| StoreError::ReadOnly)?;
        if !card.mode().writable() {
            return Err(StoreError::ReadOnly);
        }
        if card.store_id() != self.store {
            return Err(StoreError::Invalid);
        }
        let mut found = None;
        for entry in card.entries() {
            let entry = entry?;
            if entry.id == self.id && !entry.flags.has(EntryFlags::RETAINED) {
                found = Some(entry);
            }
        }
        let entry = found.ok_or(StoreError::NotFound)?;
        if entry.kind != ObjectKind::Ride || entry.flags != EntryFlags::RECORDING {
            return Err(StoreError::Invalid);
        }
        if entry.revision != self.revision {
            return Err(StoreError::RevisionConflict { current: entry.revision });
        }
        Ok(entry)
    }
}

pub struct FlatRideRecorder {
    owner: HostStore,
    ride: RideWriter,
    key: Option<Key>,
    buffers: WriteBuffers,
}

impl FlatRideRecorder {
    /// Settle a recovered footer before feeding the catalog to App. A failed publication stops
    /// startup rather than offering Continue for a finished ride.
    pub fn new(owner: HostStore) -> Result<Self, StoreError> {
        let (ride, key) = {
            let mut mounted = owner.0.lock().map_err(|_| StoreError::Media)?;
            let card = mounted.ready()?;
            let mut recording = None;
            for entry in card.entries() {
                let entry = entry?;
                if entry.flags == EntryFlags::RECORDING
                    && (entry.kind != ObjectKind::Ride || recording.replace(entry).is_some())
                {
                    return Err(StoreError::Invalid);
                }
            }
            let (ride, key) = if let Some(meta) = recording {
                let key = Key { store: card.store_id(), id: meta.id, revision: meta.revision };
                let ride = match card.recovered_ride() {
                    Some(recovered) if (recovered.id, recovered.revision) == (key.id, key.revision) => {
                        RideWriter::recover(
                            meta.name.as_str().unwrap_or(""),
                            Recovery {
                                payload_len: recovered.payload_len(),
                                payload_crc: recovered.payload_crc,
                                checkpoint_sequence: recovered.checkpoint_sequence,
                                resume: &recovered.resume,
                            },
                            |at, bytes| card.read_recovered(at, bytes),
                        )?
                    }
                    _ => RideWriter::damaged(RideDamage::Catalog),
                };
                (ride, Some(key))
            } else {
                if card.recovered_ride().is_some() {
                    return Err(StoreError::Invalid);
                }
                (RideWriter::idle(), None)
            };
            mounted.confirm_durable()?;
            if let Some(key) = key {
                key.check(&mounted)?;
            }
            (ride, key)
        };
        let mut recorder = Self { owner, ride, key, buffers: WriteBuffers::ZERO };
        if recorder.ride.is_closing() {
            recorder.flush()?;
        }
        Ok(recorder)
    }

    pub fn is_idle(&self) -> bool {
        self.ride.is_idle()
    }

    pub fn offer_recovery(&self, app: &mut App) {
        if let Some(continuation) = self.recovered_continuation() {
            app.offer_recovered_ride(continuation);
        }
        if let Some(damage) = self.ride.recovery_damage() {
            app.offer_damaged_ride(damage);
        }
    }

    pub fn recovered_continuation(&self) -> Option<RideContinuation> {
        self.ride.recovered_continuation()
    }

    fn start(&mut self, session: u32, name: &str) -> Result<(), StoreError> {
        let mut owner = self.owner.0.lock().map_err(|_| StoreError::Media)?;
        let card = owner.ready()?;
        let mut count = 0;
        for entry in card.entries() {
            let entry = entry?;
            if entry.flags == EntryFlags::RECORDING {
                return Err(StoreError::Busy);
            }
            if entry.kind == ObjectKind::Ride && entry.flags == EntryFlags::NONE {
                count += 1;
            }
        }
        if count >= obc_app::MAX_RIDES {
            return Err(StoreError::Busy);
        }
        let key = Key { store: card.store_id(), id: card.next_object_id(), revision: Revision(1) };
        let name = DisplayName::new(name).unwrap_or_default();
        let allocation = card.allocate(RESERVE_BYTES)?;
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
        if let Err(error) = owner.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]) {
            // An uncertain publication retains its reservation until remount.
            if error == StoreError::Media {
                self.key = Some(key);
                self.ride = RideWriter::damaged(RideDamage::Catalog);
            } else {
                owner.card.cancel(allocation);
            }
            return Err(error);
        }
        self.key = Some(key);
        self.ride = RideWriter::start(name.as_str().unwrap_or(""), Some(session));
        Ok(())
    }

    fn flush(&mut self) -> Result<(), StoreError> {
        while let Some(write) = self.ride.pending_write() {
            let key = self.key.ok_or(StoreError::Invalid)?;
            let mut owner = self.owner.0.lock().map_err(|_| StoreError::Media)?;
            key.check(&owner)?;
            match write {
                Write::Journal { append_len, payload_crc } => owner.card.journal(RideCheckpoint {
                    id: key.id,
                    revision: key.revision,
                    append: &self.buffers.append[..append_len],
                    payload_crc,
                    resume: &self.buffers.resume,
                })?,
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
                    owner.commit(&[Mutation::Put { meta, source: PutSource::Amend }])?;
                }
                Write::Remove => {
                    owner.commit(&[Mutation::Remove { id: key.id, revision: key.revision }])?;
                }
            }
            self.ride.acknowledge(&mut self.buffers);
            if self.ride.is_idle() {
                self.key = None;
            }
        }
        Ok(())
    }
}

impl TrackRepository for FlatRideRecorder {
    fn open(&mut self, session: u32, name: Option<&str>, now_ms: u32) -> bool {
        if self.ride.is_idle() {
            return self.start(session, name.unwrap_or("")).is_ok();
        }
        let Ok(owner) = self.owner.0.lock() else { return false };
        let Some(key) = self.key else { return false };
        if key.check(&owner).is_err() {
            return false;
        }
        self.ride.attach(session, name.unwrap_or(""), now_ms)
    }

    fn append_batch(
        &mut self,
        points: &[TrackPoint],
        boundary: Option<RideContinuation>,
    ) -> Result<AppendStatus, RecorderError> {
        let Some(boundary) = boundary else { return Ok(AppendStatus::Cancelled) };
        let owner = self.owner.0.lock().map_err(|_| RecorderError::ReadOnly)?;
        self.key.ok_or(RecorderError::Write)?.check(&owner).map_err(recorder_error)?;
        match self.ride.append(&mut self.buffers, points, boundary) {
            AppendResult::Accepted => Ok(AppendStatus::Accepted(points.len() as u16)),
            AppendResult::NeedsCheckpoint => Ok(AppendStatus::NeedsCheckpoint),
            AppendResult::Failed => Err(RecorderError::Write),
        }
    }

    fn checkpoint(
        &mut self,
        stats: RideStats,
        boundary: Option<RideContinuation>,
    ) -> Result<CheckpointStatus, RecorderError> {
        if !self.ride.checkpoint(&mut self.buffers, &stats, boundary) {
            return Err(RecorderError::Write);
        }
        self.flush().map_err(recorder_error)?;
        let owner = self.owner.0.lock().map_err(|_| RecorderError::ReadOnly)?;
        Ok(if owner.persistent { CheckpointStatus::Durable } else { CheckpointStatus::Unsupported })
    }

    fn finalize(&mut self, stats: RideStats) -> RideClose {
        if self.ride.is_idle() {
            return RideClose::Nothing;
        }
        self.ride.finish(&mut self.buffers, &stats);
        if !self.ride.is_closing() {
            return RideClose::Failed;
        }
        let Some(key) = self.key else { return RideClose::Failed };
        match self.flush() {
            Ok(()) => RideClose::Committed(key.id.0),
            Err(_) => RideClose::Failed,
        }
    }

    fn discard(&mut self) -> Result<(), RecorderError> {
        self.ride.discard();
        self.flush().map_err(recorder_error)
    }

    fn append(&mut self, _point: TrackPoint) -> bool {
        false
    }
}

fn recorder_error(error: StoreError) -> RecorderError {
    match error {
        StoreError::ReadOnly | StoreError::Invalid | StoreError::RevisionConflict { .. } | StoreError::NotFound => {
            RecorderError::ReadOnly
        }
        _ => RecorderError::Write,
    }
}

#[cfg(test)]
mod tests;
