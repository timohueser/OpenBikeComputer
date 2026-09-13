//! Physical ride writes on the shared card. Recorder owns admission and the recovery decision.

use obc_app::recorder::{continuation, CheckpointStatus, RecorderError, RideClose, RideContinuation};
use obc_app::{App, RideDamage};
use obc_crc::Crc32;
use obc_formats::ride::{decode_footer, FOOTER_LEN, SAMPLE_LEN};
use obc_ports::TrackPoint;
use obc_route::RideStats;
use obc_storage::flat::{
    DisplayName, EntryFlags, EntryMeta, Mutation, ObjectId, ObjectKind, PutSource, Revision, RideCheckpoint, Store,
    StoreError, StoreId,
};

use crate::flat_store::{HostStore, MountedStore};
use crate::repo::{AppendStatus, TrackRepository};

const RESERVE: u64 = 32 * 1024 * 1024;
const SAMPLES: usize = 16;
const DELTA: usize = SAMPLES * SAMPLE_LEN + FOOTER_LEN;

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
            if entry.id == self.id && !entry.flags.has(EntryFlags::RETAINED) {
                found = Some(entry);
            }
        }
        if !card.entries_ok() {
            return Err(StoreError::Media);
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

struct Live {
    key: Key,
    name: DisplayName,
    session: Option<u32>,
    points: u32,
    first: Option<u32>,
    last: Option<u32>,
    start: Option<u32>,
    same_boot: bool,
    rebase: Option<(u32, u32)>,
    continuation: RideContinuation,
    crc: Crc32,
    len: usize,
    /// The failed journal tuple includes these exact opaque bytes, plus the unchanged delta/CRC.
    pending: Option<[u8; continuation::RIDE_RESUME_LEN]>,
}

struct Closing {
    live: Live,
    footer: [u8; FOOTER_LEN],
    appended: bool,
    journaled: bool,
}

enum State {
    Idle,
    Live(Live),
    Closing(Closing),
    Damaged(Key, RideDamage),
}

pub struct FlatRideRecorder {
    owner: HostStore,
    state: State,
    delta: [u8; DELTA],
}

impl FlatRideRecorder {
    /// Reconcile before the first catalog/App feed. An interrupted footer is terminal and must
    /// settle here; failure stops startup rather than offering Continue for a finished ride.
    pub fn new(owner: HostStore) -> Result<Self, StoreError> {
        let state = {
            let mut mounted = owner.0.lock().map_err(|_| StoreError::Media)?;
            let card = mounted.ready()?;
            let mut recording = None;
            for entry in card.entries() {
                if entry.flags == EntryFlags::RECORDING
                    && (entry.kind != ObjectKind::Ride || recording.replace(entry).is_some())
                {
                    return Err(StoreError::Invalid);
                }
            }
            if !card.entries_ok() {
                return Err(StoreError::Media);
            }
            if let Some(meta) = recording {
                let key = Key { store: card.store_id(), id: meta.id, revision: meta.revision };
                let state = recover(&mounted, key, meta.name)?;
                mounted.confirm_durable()?;
                key.check(&mounted)?;
                state
            } else {
                if card.recovered_ride().is_some() {
                    return Err(StoreError::Invalid);
                }
                mounted.confirm_durable()?;
                State::Idle
            }
        };
        let mut recorder = Self { owner, state, delta: [0; DELTA] };
        if matches!(recorder.state, State::Closing(_)) {
            recorder.finish()?;
        }
        Ok(recorder)
    }

    pub fn offer_recovery(&self, app: &mut App) {
        match &self.state {
            State::Live(live) if live.session.is_none() => app.offer_recovered_ride(live.continuation),
            State::Damaged(_, damage) => app.offer_damaged_ride(*damage),
            _ => false,
        };
    }

    pub fn recovered_continuation(&self) -> Option<RideContinuation> {
        match &self.state {
            State::Live(live) if live.session.is_none() => Some(live.continuation),
            _ => None,
        }
    }

    fn start(&mut self, session: u32, name: &str) -> Result<(), StoreError> {
        let mut owner = self.owner.0.lock().map_err(|_| StoreError::Media)?;
        let card = owner.ready()?;
        let mut count = 0;
        for entry in card.entries() {
            if entry.flags == EntryFlags::RECORDING {
                return Err(StoreError::Busy);
            }
            if entry.kind == ObjectKind::Ride && entry.flags == EntryFlags::NONE {
                count += 1;
            }
        }
        if !card.entries_ok() {
            return Err(StoreError::Media);
        }
        if count >= obc_app::MAX_RIDES {
            return Err(StoreError::Busy);
        }
        let key = Key { store: card.store_id(), id: card.next_object_id(), revision: Revision(1) };
        let name = DisplayName::new(name).unwrap_or_default();
        let allocation = card.allocate(RESERVE)?;
        let meta = EntryMeta {
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
                self.state = State::Damaged(key, RideDamage::Catalog);
            } else {
                owner.card.cancel(allocation);
            }
            return Err(error);
        }
        self.delta.fill(0);
        self.state = State::Live(Live {
            key,
            name,
            session: Some(session),
            points: 0,
            first: None,
            last: None,
            start: None,
            same_boot: true,
            rebase: None,
            continuation: RideContinuation::default(),
            crc: Crc32::new(),
            len: 0,
            pending: None,
        });
        Ok(())
    }

    fn finish(&mut self) -> Result<ObjectId, StoreError> {
        let State::Closing(closing) = &mut self.state else { return Err(StoreError::Invalid) };
        let live = &mut closing.live;
        if live.pending.is_some() && !closing.appended {
            flush(&self.owner, live, &self.delta)?;
        }
        if !closing.appended {
            self.delta[live.len..live.len + FOOTER_LEN].copy_from_slice(&closing.footer);
            live.len += FOOTER_LEN;
            live.crc.update(&closing.footer);
            live.pending = Some(continuation::encode(live.continuation, live.start));
            closing.appended = true;
        }
        if !closing.journaled {
            flush(&self.owner, live, &self.delta)?;
            closing.journaled = true;
        }
        let mut owner = self.owner.0.lock().map_err(|_| StoreError::Media)?;
        live.key.check(&owner)?;
        let meta = EntryMeta {
            id: live.key.id,
            revision: live.key.revision,
            kind: ObjectKind::Ride,
            flags: EntryFlags::NONE,
            payload_len: u64::from(live.points) * SAMPLE_LEN as u64 + FOOTER_LEN as u64,
            payload_crc: live.crc.finalize(),
            name: live.name,
        };
        owner.commit(&[Mutation::Put { meta, source: PutSource::Amend }])?;
        let id = live.key.id;
        self.state = State::Idle;
        Ok(id)
    }
}

impl TrackRepository for FlatRideRecorder {
    fn open(&mut self, session: u32, name: Option<&str>, now_ms: u32) -> bool {
        if let State::Live(live) = &self.state {
            let Ok(owner) = self.owner.0.lock() else { return false };
            if live.key.check(&owner).is_err() {
                return false;
            }
        }
        match &mut self.state {
            State::Idle => self.start(session, name.unwrap_or("")).is_ok(),
            State::Live(live) => {
                if live.session.is_none() {
                    live.session = Some(session);
                    live.rebase = live.last.map(|last| (now_ms, last));
                }
                live.session == Some(session)
            }
            _ => false,
        }
    }

    fn append_batch(
        &mut self,
        points: &[TrackPoint],
        boundary: Option<RideContinuation>,
    ) -> Result<AppendStatus, RecorderError> {
        let State::Live(live) = &mut self.state else { return Err(RecorderError::Write) };
        let Some(continuation) = boundary else { return Ok(AppendStatus::Cancelled) };
        if live.session.is_none() || points.len() > SAMPLES {
            return Ok(AppendStatus::Cancelled);
        }
        let count = live.points.checked_add(points.len() as u32).ok_or(RecorderError::Write)?;
        if live.pending.is_some() || live.len + points.len() * SAMPLE_LEN + FOOTER_LEN > DELTA {
            return Ok(AppendStatus::NeedsCheckpoint);
        }
        if u64::from(count) * SAMPLE_LEN as u64 + FOOTER_LEN as u64 > RESERVE {
            return Err(RecorderError::Write);
        }
        let owner = self.owner.0.lock().map_err(|_| RecorderError::ReadOnly)?;
        live.key.check(&owner).map_err(recorder_error)?;
        for point in points {
            let mut point = *point;
            if let Some((source, logical)) = live.rebase {
                point.t_ms = logical.wrapping_add(point.t_ms.wrapping_sub(source));
            }
            let bytes = obc_formats::track::encode_record(&point);
            self.delta[live.len..live.len + SAMPLE_LEN].copy_from_slice(&bytes);
            live.len += SAMPLE_LEN;
            live.crc.update(&bytes);
            live.first.get_or_insert(point.t_ms);
            live.last = Some(point.t_ms);
        }
        live.points = count;
        live.continuation = continuation;
        Ok(AppendStatus::Accepted(points.len() as u16))
    }

    fn checkpoint(
        &mut self,
        stats: RideStats,
        boundary: Option<RideContinuation>,
    ) -> Result<CheckpointStatus, RecorderError> {
        let State::Live(live) = &mut self.state else { return Err(RecorderError::Write) };
        if live.pending.is_none() {
            live.start = stable_start(live, &stats);
            live.continuation = boundary.unwrap_or(live.continuation);
            live.pending = Some(continuation::encode(live.continuation, live.start));
        }
        flush(&self.owner, live, &self.delta).map_err(recorder_error)?;
        Ok(CheckpointStatus::Durable)
    }

    fn finalize(&mut self, stats: RideStats) -> RideClose {
        if matches!(self.state, State::Idle) {
            return RideClose::Nothing;
        }
        if matches!(self.state, State::Live(_)) {
            let State::Live(mut live) = std::mem::replace(&mut self.state, State::Idle) else { unreachable!() };
            let start = stable_start(&live, &stats);
            let mut footer_stats = stats;
            footer_stats.unix_at_anchor = start.unwrap_or(0);
            footer_stats.anchor_ms = live.first.unwrap_or(0);
            footer_stats.clock_trusted = start.is_some();
            let footer = obc_route::encode_summary_footer(
                live.name.as_str().unwrap_or(""),
                &footer_stats,
                live.points,
                live.first,
            );
            // An outstanding checkpoint keeps its frozen start until that exact tuple succeeds.
            if live.pending.is_none() {
                live.start = start;
            }
            self.state = State::Closing(Closing { live, footer, appended: false, journaled: false });
        }
        match self.finish() {
            Ok(id) => RideClose::Committed(id.0),
            Err(_) => RideClose::Failed,
        }
    }

    fn discard(&mut self) -> Result<(), RecorderError> {
        let key = match &self.state {
            State::Idle => return Ok(()),
            State::Live(live) => live.key,
            State::Closing(closing) => closing.live.key,
            State::Damaged(key, _) => *key,
        };
        let mut owner = self.owner.0.lock().map_err(|_| RecorderError::ReadOnly)?;
        key.check(&owner).map_err(recorder_error)?;
        owner.commit(&[Mutation::Remove { id: key.id, revision: key.revision }]).map_err(recorder_error)?;
        self.state = State::Idle;
        Ok(())
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

fn stable_start(live: &Live, stats: &RideStats) -> Option<u32> {
    live.start.or_else(|| {
        (live.same_boot && stats.clock_trusted).then(|| {
            stats
                .unix_at_anchor
                .wrapping_sub(stats.anchor_ms.wrapping_sub(live.first.unwrap_or(stats.anchor_ms)) / 1000)
        })
    })
}

fn flush(owner: &HostStore, live: &mut Live, delta: &[u8; DELTA]) -> Result<(), StoreError> {
    let owner = owner.0.lock().map_err(|_| StoreError::Media)?;
    live.key.check(&owner)?;
    owner.card.journal(RideCheckpoint {
        id: live.key.id,
        revision: live.key.revision,
        append: &delta[..live.len],
        payload_crc: live.crc.finalize(),
        resume: live.pending.as_ref().ok_or(StoreError::Invalid)?,
    })?;
    live.pending = None;
    live.len = 0;
    Ok(())
}

fn recover(owner: &MountedStore, key: Key, name: DisplayName) -> Result<State, StoreError> {
    let card = owner.ready()?;
    let Some(recovered) = card.recovered_ride() else { return Ok(State::Damaged(key, RideDamage::Catalog)) };
    if (recovered.id, recovered.revision) != (key.id, key.revision) {
        return Ok(State::Damaged(key, RideDamage::Catalog));
    }
    let total = recovered.payload_len();
    let sample_time = |at| {
        let mut bytes = [0; SAMPLE_LEN];
        (card.read_recovered(at, &mut bytes).ok()? == SAMPLE_LEN)
            .then(|| obc_formats::track::decode_record(&bytes).t_ms)
    };
    let first = (total >= SAMPLE_LEN as u64).then(|| sample_time(0)).flatten();
    let live = |points, last, continuation, start| Live {
        key,
        name,
        session: None,
        points,
        first,
        last,
        start,
        same_boot: false,
        rebase: None,
        continuation,
        crc: Crc32::from_checksum(recovered.payload_crc),
        len: 0,
        pending: None,
    };
    if total >= FOOTER_LEN as u64 && (total - FOOTER_LEN as u64).is_multiple_of(SAMPLE_LEN as u64) {
        let mut bytes = [0; FOOTER_LEN];
        let at = total - FOOTER_LEN as u64;
        if card.read_recovered(at, &mut bytes)? == FOOTER_LEN {
            if let Ok(footer) = decode_footer(&bytes) {
                let points = (at / SAMPLE_LEN as u64) as u32;
                if footer.point_count == points {
                    let mut live = live(points, None, RideContinuation::default(), None);
                    live.name = DisplayName::new(footer.name()).unwrap_or_default();
                    return Ok(State::Closing(Closing { live, footer: bytes, appended: true, journaled: true }));
                }
            }
        }
    }
    let last = total.checked_sub(SAMPLE_LEN as u64).and_then(sample_time);
    if !total.is_multiple_of(SAMPLE_LEN as u64) || (total != 0 && (first.is_none() || last.is_none())) {
        return Ok(State::Damaged(key, RideDamage::Payload));
    }
    let decoded = if total == 0 && recovered.checkpoint_sequence == 0 && recovered.resume.iter().all(|byte| *byte == 0)
    {
        Some((RideContinuation::default(), None))
    } else {
        continuation::decode(&recovered.resume)
    };
    Ok(match decoded {
        Some((continuation, start)) => State::Live(live((total / SAMPLE_LEN as u64) as u32, last, continuation, start)),
        None => State::Damaged(key, RideDamage::Metadata),
    })
}

#[cfg(test)]
mod tests;
