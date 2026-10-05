//! Durable proof that a client holds the exact finalized ride bytes.

use super::{
    BlockDevice, EntryFlags, EntryMeta, FlatStore, Mutation, ObjectId, ObjectKind, PutSource, Revision, Store,
    StoreError, StoreId,
};

use obc_formats::assistant::{NavigatorCheckpoint, PayloadFingerprint, CHECKPOINT_LEN, CHECKPOINT_VERSION};
use obc_formats::trip_progress::{TripProgress, MAX_RECORDS, RECORD_LEN};

pub const HEADER_LEN: usize = 32;
pub const ROW_LEN: usize = 40;
pub const MAX_RIDES: usize = 128;
pub const MAX_LEN: usize = HEADER_LEN + MAX_RIDES * ROW_LEN + CHECKPOINT_LEN + MAX_RECORDS * RECORD_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Capacity,
    WrongStore,
    DuplicateObject,
    Stale,
    Store(StoreError),
    /// Commit or committed read-back failed. Reopen the card before further metadata writes.
    RemountRequired,
}

impl From<StoreError> for Error {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

/// A finalized ride archive proof, bound to exact source bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub id: ObjectId,
    pub revision: Revision,
    pub payload_len: u64,
    pub payload_crc: u32,
    pub timestamp: u32,
    pub kind: ObjectKind,
}

impl Row {
    fn valid(self) -> bool {
        self.id.0 != 0 && self.revision.0 != 0 && self.kind == ObjectKind::Ride
    }

    fn matches(self, entry: EntryMeta) -> bool {
        self.id == entry.id
            && self.revision == entry.revision
            && self.kind == entry.kind
            && self.payload_len == entry.payload_len
            && self.payload_crc == entry.payload_crc
            && entry.flags == EntryFlags::NONE
    }

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let row = Self::from_bytes(bytes);
        if !row.valid()
            || bytes[32..34] != (ObjectKind::Ride as u16).to_le_bytes()
            || bytes[34..].iter().any(|&v| v != 0)
        {
            return Err(Error::Invalid);
        }
        Ok(row)
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            id: ObjectId(u64::from_le_bytes(bytes[0..8].try_into().unwrap())),
            revision: Revision(u64::from_le_bytes(bytes[8..16].try_into().unwrap())),
            payload_len: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            payload_crc: u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            timestamp: u32::from_le_bytes(bytes[28..32].try_into().unwrap()),
            kind: ObjectKind::Ride,
        }
    }

    fn encode(self, bytes: &mut [u8]) {
        bytes.fill(0);
        bytes[0..8].copy_from_slice(&self.id.0.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.revision.0.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.payload_len.to_le_bytes());
        bytes[24..28].copy_from_slice(&self.payload_crc.to_le_bytes());
        bytes[28..32].copy_from_slice(&self.timestamp.to_le_bytes());
        bytes[32..34].copy_from_slice(&(self.kind as u16).to_le_bytes());
    }
}

/// An editable payload image, not evidence that edits are durable.
struct Image<'a> {
    buffer: &'a mut [u8],
    len: usize,
    base: Option<Option<EntryMeta>>,
}

impl<'a> Image<'a> {
    pub fn empty(store: StoreId, buffer: &'a mut [u8]) -> Result<Self, Error> {
        if buffer.len() < HEADER_LEN {
            return Err(Error::Capacity);
        }
        buffer[..HEADER_LEN].fill(0);
        buffer[..4].copy_from_slice(b"OBRM");
        buffer[4..6].copy_from_slice(&2u16.to_le_bytes());
        buffer[6..8].copy_from_slice(&(HEADER_LEN as u16).to_le_bytes());
        buffer[8..10].copy_from_slice(&(ROW_LEN as u16).to_le_bytes());
        buffer[16..32].copy_from_slice(&store.0);
        Ok(Self { buffer, len: HEADER_LEN, base: None })
    }

    pub fn decode(buffer: &'a mut [u8], len: usize) -> Result<Self, Error> {
        if len < HEADER_LEN || len > buffer.len() || len > MAX_LEN {
            return Err(Error::Invalid);
        }
        Layout::parse(len, &mut |offset, out| {
            out.copy_from_slice(&buffer[offset..offset + out.len()]);
            Ok(())
        })?;
        Ok(Self { buffer, len, base: None })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.buffer[..self.len]
    }
    pub fn store_id(&self) -> StoreId {
        StoreId(self.buffer[16..32].try_into().unwrap())
    }
    pub fn rows(&self) -> impl Iterator<Item = Row> + '_ {
        self.bytes()[HEADER_LEN..self.rows_end()].as_chunks::<ROW_LEN>().0.iter().map(|b| Row::from_bytes(b))
    }

    fn rows_end(&self) -> usize {
        HEADER_LEN + u16::from_le_bytes(self.buffer[10..12].try_into().unwrap()) as usize * ROW_LEN
    }

    fn progress_start(&self) -> usize {
        self.rows_end() + u16::from_le_bytes(self.buffer[14..16].try_into().unwrap()) as usize
    }

    pub fn checkpoint(&self) -> Option<NavigatorCheckpoint> {
        NavigatorCheckpoint::decode(&self.bytes()[self.rows_end()..self.progress_start()])
    }

    /// The trip progress records, in write order.
    #[cfg(test)]
    pub fn progress(&self) -> impl Iterator<Item = TripProgress> + '_ {
        let records = self.bytes()[self.progress_start()..].as_chunks::<RECORD_LEN>().0;
        records.iter().map(|b| TripProgress::decode(b).unwrap())
    }

    /// Replace every trip progress record. The keys must be unique and nonzero.
    #[cfg(test)]
    pub fn set_progress(&mut self, records: &[TripProgress]) -> Result<(), Error> {
        let start = self.progress_start();
        let unique = records.iter().enumerate().all(|(i, r)| r.key != 0 && records[..i].iter().all(|o| o.key != r.key));
        if records.len() > MAX_RECORDS || !unique {
            return Err(Error::Invalid);
        }
        if start + records.len() * RECORD_LEN > self.buffer.len() {
            return Err(Error::Capacity);
        }
        for (out, record) in self.buffer[start..].as_chunks_mut::<RECORD_LEN>().0.iter_mut().zip(records) {
            *out = record.encode();
        }
        self.len = start + records.len() * RECORD_LEN;
        Ok(())
    }

    pub fn set_checkpoint(&mut self, checkpoint: Option<NavigatorCheckpoint>) -> Result<(), Error> {
        let start = self.rows_end();
        let encoded = match checkpoint {
            Some(value) => Some(value.encode().ok_or(Error::Invalid)?),
            None => None,
        };
        let (old_end, new_end) = (self.progress_start(), start + encoded.map_or(0, |_| CHECKPOINT_LEN));
        let len = self.len - old_end + new_end;
        if len > self.buffer.len() {
            return Err(Error::Capacity);
        }
        self.buffer.copy_within(old_end..self.len, new_end);
        self.len = len;
        if let Some(bytes) = encoded {
            self.buffer[start..new_end].copy_from_slice(&bytes);
            self.buffer[12..14].copy_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
            self.buffer[14..16].copy_from_slice(&(CHECKPOINT_LEN as u16).to_le_bytes());
        } else {
            self.buffer[12..16].fill(0);
        }
        Ok(())
    }

    pub fn set(&mut self, row: Row) -> Result<(), Error> {
        if !row.valid() {
            return Err(Error::Invalid);
        }
        let count = self.rows().count();
        let mut index = 0;
        while index < count {
            let at = HEADER_LEN + index * ROW_LEN;
            if u64::from_le_bytes(self.buffer[at..at + 8].try_into().unwrap()) >= row.id.0 {
                break;
            }
            index += 1;
        }
        let at = HEADER_LEN + index * ROW_LEN;
        let existing = index < count && u64::from_le_bytes(self.buffer[at..at + 8].try_into().unwrap()) == row.id.0;
        if !existing && (count == MAX_RIDES || self.len + ROW_LEN > self.buffer.len()) {
            return Err(Error::Capacity);
        }
        if !existing {
            self.buffer.copy_within(at..self.len, at + ROW_LEN);
            self.len += ROW_LEN;
            self.update_count(count + 1);
        }
        row.encode(&mut self.buffer[at..at + ROW_LEN]);
        Ok(())
    }

    fn update_count(&mut self, count: usize) {
        self.buffer[10..12].copy_from_slice(&(count as u16).to_le_bytes());
    }

    fn matching_rows<D: BlockDevice>(
        &self,
        store: &FlatStore<D>,
        target: Option<&EntryMeta>,
    ) -> Result<([bool; MAX_RIDES], bool), Error> {
        let rows = self.bytes()[HEADER_LEN..self.rows_end()].as_chunks::<ROW_LEN>().0;
        scan_rows(store, rows.len(), &mut |i| Ok(Row::from_bytes(&rows[i])), target)
    }

    /// Remove stale rows only after a complete, successful catalog scan.
    pub fn reconcile<D: BlockDevice>(&mut self, store: &FlatStore<D>) -> Result<bool, Error> {
        if self.store_id() != store.store_id() {
            return Err(Error::WrongStore);
        }
        let (keep, _) = self.matching_rows(store, None)?;
        let old_len = self.len;
        let rows_end = self.rows_end();
        let mut dest = HEADER_LEN;
        for (index, &keep) in keep.iter().take((rows_end - HEADER_LEN) / ROW_LEN).enumerate() {
            if keep {
                let at = HEADER_LEN + index * ROW_LEN;
                self.buffer.copy_within(at..at + ROW_LEN, dest);
                dest += ROW_LEN;
            }
        }
        self.buffer.copy_within(rows_end..old_len, dest);
        self.len = dest + old_len - rows_end;
        self.update_count((dest - HEADER_LEN) / ROW_LEN);
        Ok(self.len != old_len)
    }
}

#[inline(never)]
fn scan_rows<D: BlockDevice>(
    store: &FlatStore<D>,
    rows: usize,
    read: &mut dyn FnMut(usize) -> Result<Row, Error>,
    target: Option<&EntryMeta>,
) -> Result<([bool; MAX_RIDES], bool), Error> {
    let mut found = [false; MAX_RIDES];
    let mut present = false;
    let mut i = 0;
    let mut row = if rows != 0 { Some(read(0)?) } else { None };
    for entry in store.entries() {
        while row.is_some_and(|row| row.id < entry.id) {
            i += 1;
            row = if i < rows { Some(read(i)?) } else { None };
        }
        if let Some(row) = row {
            let matched = row.matches(entry);
            found[i] |= matched;
            present |= matched && target == Some(&entry);
        }
    }
    if !store.entries_ok() {
        return Err(Error::Store(StoreError::Media));
    }
    Ok((found, present))
}

fn check_mode<D: BlockDevice>(store: &FlatStore<D>) -> Result<(), Error> {
    if store.mode() == super::Mode::RemountRequired {
        return Err(Error::RemountRequired);
    }
    Ok(())
}

fn load_image<'a, D: BlockDevice>(store: &FlatStore<D>, buffer: &'a mut [u8]) -> Result<Image<'a>, Error> {
    check_mode(store)?;
    let head = singleton(store)?;
    let mut image = if let Some(head) = head {
        let len = usize::try_from(head.payload_len).map_err(|_| Error::Capacity)?;
        if len > buffer.len() || len > MAX_LEN {
            return Err(Error::Capacity);
        }
        read_payload(store, head, &mut buffer[..len])?;
        Image::decode(buffer, len)?
    } else {
        Image::empty(store.store_id(), buffer)?
    };
    if image.store_id() != store.store_id() {
        return Err(Error::WrongStore);
    }
    image.base = Some(head);
    Ok(image)
}

#[inline(never)]
fn publish_image<D: BlockDevice>(
    store: &FlatStore<D>,
    image: &mut Image<'_>,
    target: Option<EntryMeta>,
    accepted: Option<EntryMeta>,
) -> Result<EntryMeta, Error> {
    check_mode(store)?;
    let identity = image.store_id();
    if identity != store.store_id() {
        return Err(Error::WrongStore);
    }
    let head = image.base.ok_or(Error::Stale)?;
    if singleton(store)? != head {
        return Err(Error::Stale);
    }
    let (found, target_present) = image.matching_rows(store, target.as_ref())?;
    if found.iter().take(image.rows().count()).any(|&v| !v) || (target.is_some() && !target_present) {
        return Err(Error::Stale);
    }
    let (id, revision) = match head {
        Some(head) => (head.id, Revision(head.revision.0.checked_add(1).ok_or(Error::Invalid)?)),
        None => (store.next_object_id(), Revision(1)),
    };
    let meta = EntryMeta {
        added_at_utc: 0,
        id,
        revision,
        kind: ObjectKind::Metadata,
        flags: EntryFlags::NONE,
        payload_len: image.len as u64,
        payload_crc: obc_crc::crc32(image.bytes()),
        name: Default::default(),
    };
    if !store.has_open_capacity() {
        return Err(Error::Store(StoreError::Busy));
    }
    let sequence = store.sequence();
    let mut allocation = store.allocate(meta.payload_len)?;
    if let Err(error) = store.write(&mut allocation, image.bytes()) {
        store.cancel(allocation);
        return Err(error.into());
    }
    if store.store_id() != identity || store.sequence() != sequence {
        store.cancel(allocation);
        return Err(Error::Stale);
    }
    let put = Mutation::Put { meta, source: PutSource::Fresh(allocation) };
    let mut batch = heapless::Vec::<Mutation, 3>::new();
    if let Some(old) = head {
        batch.push(Mutation::Remove { id: old.id, revision: old.revision }).unwrap();
    }
    batch.push(put).unwrap();
    if let Some(meta) = accepted {
        batch.push(Mutation::Put { meta, source: PutSource::Amend }).unwrap();
    }
    let result = store.commit(&batch);
    if let Err(error) = result {
        if store.mode() == super::Mode::RemountRequired {
            return Err(Error::RemountRequired);
        }
        store.cancel(allocation);
        return Err(error.into());
    }
    verify_published(store, meta, image.bytes(), accepted)?;
    image.base = Some(Some(meta));
    Ok(meta)
}

#[inline(never)]
fn verify_published<D: BlockDevice>(
    store: &FlatStore<D>,
    meta: EntryMeta,
    bytes: &[u8],
    accepted: Option<EntryMeta>,
) -> Result<(), Error> {
    let handle = store.open(meta.id, Some(meta.revision));
    let verified = handle.and_then(|handle| {
        let mut block = [0u8; 512];
        let result = bytes.chunks(512).enumerate().try_for_each(|(i, want)| {
            let got = store.read(&handle, (i * 512) as u64, &mut block[..want.len()])?;
            if got != want.len() || &block[..got] != want {
                return Err(StoreError::Media);
            }
            Ok(())
        });
        store.close(handle);
        result
    });
    let acceptance_verified = accepted.is_none_or(|want| {
        let found = store.entries().any(|entry| entry == want);
        found && store.entries_ok()
    });
    if verified.is_err() || !acceptance_verified {
        store.require_remount();
        return Err(Error::RemountRequired);
    }
    Ok(())
}

fn singleton<D: BlockDevice>(store: &FlatStore<D>) -> Result<Option<EntryMeta>, Error> {
    let mut found = None;
    let mut duplicate = false;
    for entry in store.entries().filter(|e| e.kind == ObjectKind::Metadata && !e.flags.has(EntryFlags::RETAINED)) {
        duplicate |= found.is_some() || entry.flags != EntryFlags::NONE;
        found = Some(entry);
    }
    if !store.entries_ok() {
        return Err(Error::Store(StoreError::Media));
    }
    if duplicate {
        return Err(Error::DuplicateObject);
    }
    Ok(found)
}

fn read_payload<D: BlockDevice>(store: &FlatStore<D>, meta: EntryMeta, bytes: &mut [u8]) -> Result<(), Error> {
    let handle = store.open(meta.id, Some(meta.revision))?;
    let read = store.read(&handle, 0, bytes);
    store.close(handle);
    if read? != bytes.len() || obc_crc::crc32(bytes) != meta.payload_crc {
        return Err(Error::Invalid);
    }
    Ok(())
}

mod read;
use read::{Layout, Reader};

#[cfg(test)]
mod differential;
#[cfg(test)]
mod tests;

/// Prune only from a complete catalog; absence does not create a metadata object.
#[inline(never)]
pub fn reconcile<D: BlockDevice>(store: &FlatStore<D>) -> Result<(), Error> {
    edit(store, Edit::Reconcile).map(|_| ())
}

/// Feed policy rows only after complete source validation and a durability barrier.
/// Stage callback values until success; a later streamed read can fail.
#[inline(never)]
pub fn read_rows<D: BlockDevice>(store: &FlatStore<D>, mut accept: impl FnMut(Row)) -> Result<(), Error> {
    let mut reader = Reader::open(store)?;
    let keep = Reader::matching_rows(store, reader.as_mut())?;
    durable(store)?;
    if let Some(reader) = reader.as_mut() {
        for (i, keep) in keep.iter().enumerate().take(reader.layout.rows) {
            if *keep {
                accept(reader.row(i)?);
            }
        }
    }
    Ok(())
}
/// Every proof row as the last commit left it, with no reconcile and no barrier. Policy reads
/// [`read_rows`]; this reads back what a receipt committed, so an observer sees the same identity
/// and stamp a remount would.
///
/// A card with no metadata object censuses as zero rows, the same answer as a card whose metadata
/// holds none. The returned [`StoreId`] is always the mounted store's: a foreign image is a
/// `WrongStore` error, never a row. Callback values are tentative until success.
#[inline(never)]
pub fn census<D: BlockDevice>(store: &FlatStore<D>, mut accept: impl FnMut(Row)) -> Result<StoreId, Error> {
    if let Some(mut reader) = Reader::open(store)? {
        for i in 0..reader.layout.rows {
            accept(reader.row(i)?);
        }
    }
    Ok(store.store_id())
}

fn durable<D: BlockDevice>(store: &FlatStore<D>) -> Result<(), Error> {
    // A live-medium remount can read a gate whose previous final sync failed.
    if store.sync_media().is_err() {
        store.require_remount();
        return Err(Error::RemountRequired);
    }
    Ok(())
}
/// Persist possession of the exact current finalized ride. Duplicate receipts preserve the stamp
/// and make no commit. The serialized writer supplies exclusivity for the whole operation.
#[inline(never)]
pub fn archive_ride<D: BlockDevice>(
    store: &FlatStore<D>,
    expected: StoreId,
    id: ObjectId,
    revision: Revision,
    payload_len: u64,
    payload_crc: u32,
) -> Result<u32, Error> {
    if store.store_id() != expected || expected.0 == [0; 16] {
        return Err(Error::WrongStore);
    }
    if store.mode() == super::Mode::RemountRequired {
        return Err(Error::RemountRequired);
    }
    if id.0 == 0 || revision.0 == 0 || payload_len == 0 {
        return Err(Error::Stale);
    }
    let target = store.entries().find(|entry| {
        entry.id == id
            && entry.revision == revision
            && entry.kind == ObjectKind::Ride
            && entry.flags == EntryFlags::NONE
            && entry.payload_len == payload_len
            && entry.payload_crc == payload_crc
    });
    if !store.entries_ok() {
        return Err(Error::Store(StoreError::Media));
    }
    let target = target.ok_or(Error::Stale)?;
    edit(store, Edit::Archive(target))
}

/// The full immutable source tuple; its StoreId comes from the Metadata image.
pub fn fingerprint(entry: EntryMeta) -> PayloadFingerprint {
    PayloadFingerprint {
        object: entry.id.0,
        revision: entry.revision.0,
        length: entry.payload_len,
        crc: entry.payload_crc,
    }
}

fn checkpoint_source<D: BlockDevice>(store: &FlatStore<D>, source: PayloadFingerprint) -> Result<EntryMeta, Error> {
    let entry = source_head(store, ObjectId(source.object), ObjectKind::Route)?;
    if fingerprint(entry) != source {
        return Err(Error::Stale);
    }
    Ok(entry)
}

fn validate_checkpoint<D: BlockDevice>(
    store: &FlatStore<D>,
    checkpoint: Option<NavigatorCheckpoint>,
    require_accepted: bool,
) -> Result<Option<EntryMeta>, Error> {
    let Some(checkpoint) = checkpoint else { return Ok(None) };
    let route = checkpoint_source(store, checkpoint.route)?;
    if require_accepted && !route.flags.has(EntryFlags::ASSISTANT_ACCEPTED) {
        return Err(Error::Invalid);
    }
    if let Some(original) = checkpoint.original {
        checkpoint_source(store, original)?;
    }
    Ok(Some(route))
}

/// Serialize this with archive proof updates. No draft survives the call.
/// A changed prior checkpoint is stale even when an unrelated metadata write was reconciled.
#[inline(never)]
pub fn write_checkpoint<D: BlockDevice>(
    store: &FlatStore<D>,
    expected_store: StoreId,
    sequence: u64,
    expected: Option<NavigatorCheckpoint>,
    next: Option<NavigatorCheckpoint>,
) -> Result<(), Error> {
    check_scope(store, expected_store, sequence)?;
    edit(store, Edit::Checkpoint { expected, next }).map(|_| ())
}

/// Write one trip progress record by the bound rules of
/// [`record`](obc_formats::trip_progress::record); `stored` says whether a stored trip holds a key.
/// The record takes the current Revision of its day route, so a later replace voids its metres. A
/// start record that finds its key's record moves that record as stored.
#[inline(never)]
pub fn write_progress<D: BlockDevice>(
    store: &FlatStore<D>,
    mut new: TripProgress,
    stored: impl Fn(u64) -> bool,
) -> Result<(), Error> {
    new.day_route.revision = route_revision(store, new.day_route.id)?.unwrap_or(0);
    edit(store, Edit::Progress { new, stored: &stored }).map(|_| ())
}

enum Edit<'a> {
    Reconcile,
    Archive(EntryMeta),
    Checkpoint { expected: Option<NavigatorCheckpoint>, next: Option<NavigatorCheckpoint> },
    Progress { new: TripProgress, stored: &'a dyn Fn(u64) -> bool },
}

// All mutators share this buffer. The serialized storage writer owns the complete call.
#[inline(never)]
fn edit<D: BlockDevice>(store: &FlatStore<D>, edit: Edit<'_>) -> Result<u32, Error> {
    let mut bytes = [0; MAX_LEN];
    let mut image = load_image(store, &mut bytes)?;
    let (target, accepted) = match edit {
        Edit::Reconcile => {
            if !image.reconcile(store)? {
                return Ok(0);
            }
            (None, None)
        }
        Edit::Archive(target) => {
            image.reconcile(store)?;
            if let Some(row) = image.rows().find(|row| row.matches(target)) {
                durable(store)?;
                return Ok(row.timestamp);
            }
            if !store.mode().writable() {
                return Err(Error::Store(StoreError::ReadOnly));
            }
            image.set(Row {
                id: target.id,
                revision: target.revision,
                payload_len: target.payload_len,
                payload_crc: target.payload_crc,
                timestamp: 0,
                kind: ObjectKind::Ride,
            })?;
            (Some(target), None)
        }
        Edit::Checkpoint { expected, next } => {
            if image.checkpoint() != expected {
                return Err(Error::Stale);
            }
            let accepted = validate_checkpoint(store, next, false)?
                .filter(|entry| !entry.flags.has(EntryFlags::ASSISTANT_ACCEPTED))
                .map(|entry| EntryMeta { flags: EntryFlags::ASSISTANT_ACCEPTED, ..entry });
            if expected == next {
                verify_checkpoint_payloads(store, next)?;
                durable(store)?;
                return Ok(0);
            }
            image.reconcile(store)?;
            image.set_checkpoint(next)?;
            (None, accepted)
        }
        Edit::Progress { new, stored } => {
            image.reconcile(store)?;
            record_progress(&mut image, new, stored)?;
            (None, None)
        }
    };
    publish_image(store, &mut image, target, accepted)?;
    Ok(0)
}

#[inline(never)]
fn record_progress(image: &mut Image<'_>, new: TripProgress, stored: &dyn Fn(u64) -> bool) -> Result<(), Error> {
    let start = image.progress_start();
    let existing = (new.last_finished.is_none())
        .then(|| {
            image.bytes()[start..]
                .as_chunks::<RECORD_LEN>()
                .0
                .iter()
                .find(|record| u64::from_le_bytes(record[..8].try_into().unwrap()) == new.key)
        })
        .flatten();
    let encoded = existing.copied().unwrap_or_else(|| new.encode());
    if new.key == 0 {
        return Err(Error::Invalid);
    }
    let mut end = start;
    for at in (start..image.len).step_by(RECORD_LEN) {
        let key = u64::from_le_bytes(image.buffer[at..at + 8].try_into().unwrap());
        if key != new.key && stored(key) {
            image.buffer.copy_within(at..at + RECORD_LEN, end);
            end += RECORD_LEN;
        }
    }
    if (end - start) / RECORD_LEN == MAX_RECORDS {
        image.buffer.copy_within(start + RECORD_LEN..end, start);
        end -= RECORD_LEN;
    }
    if end + RECORD_LEN > image.buffer.len() {
        return Err(Error::Capacity);
    }
    image.buffer[end..end + RECORD_LEN].copy_from_slice(&encoded);
    image.len = end + RECORD_LEN;
    Ok(())
}

/// The trip progress records, in write order. A record whose day route has another Revision now
/// reads its metres as 0. Stage callback values until the read succeeds.
#[inline(never)]
pub fn read_progress<D: BlockDevice>(store: &FlatStore<D>, mut accept: impl FnMut(TripProgress)) -> Result<(), Error> {
    if let Some(mut reader) = Reader::open(store)? {
        for i in 0..reader.layout.records {
            let mut record = reader.progress(i)?;
            if route_revision(store, record.day_route.id)? != Some(record.day_route.revision) {
                record.metres = 0;
            }
            accept(record);
        }
    }
    Ok(())
}

fn route_revision<D: BlockDevice>(store: &FlatStore<D>, id: u64) -> Result<Option<u64>, Error> {
    match source_head(store, ObjectId(id), ObjectKind::Route) {
        Ok(entry) => Ok(Some(entry.revision.0)),
        Err(Error::Stale) => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_checkpoint_value<D: BlockDevice>(store: &FlatStore<D>) -> Result<Option<NavigatorCheckpoint>, Error> {
    Reader::open(store)?.map_or(Ok(None), |mut reader| reader.checkpoint())
}

/// A validated recovery offer. No checkpoint means ordinary boot behavior.
#[inline(never)]
pub fn read_checkpoint<D: BlockDevice>(store: &FlatStore<D>) -> Result<Option<NavigatorCheckpoint>, Error> {
    let checkpoint = read_checkpoint_value(store)?;
    validate_checkpoint(store, checkpoint, true)?;
    verify_checkpoint_payloads(store, checkpoint)?;
    durable(store)?;
    Ok(checkpoint)
}

#[inline(never)]
fn verify_checkpoint_payloads<D: BlockDevice>(
    store: &FlatStore<D>,
    checkpoint: Option<NavigatorCheckpoint>,
) -> Result<(), Error> {
    if let Some(checkpoint) = checkpoint {
        for source in [Some(checkpoint.route), checkpoint.original].into_iter().flatten() {
            let handle = store.open(ObjectId(source.object), Some(Revision(source.revision)))?;
            let verified = (|| {
                let mut crc = obc_crc::Crc32::new();
                let mut block = [0; 512];
                let mut offset = 0;
                while offset < source.length {
                    let want = (source.length - offset).min(block.len() as u64) as usize;
                    if store.read(&handle, offset, &mut block[..want])? != want {
                        return Err(Error::Invalid);
                    }
                    crc.update(&block[..want]);
                    offset += want as u64;
                }
                if crc.finalize() != source.crc {
                    return Err(Error::Invalid);
                }
                Ok(())
            })();
            store.close(handle);
            verified?;
        }
    }
    Ok(())
}

/// Explicit removal/replacement must preserve an accepted journey's sources.
#[inline(never)]
pub fn check_route_change<D: BlockDevice>(store: &FlatStore<D>, id: ObjectId) -> Result<(), Error> {
    let route = store.entries().any(|entry| entry.id == id && entry.kind == ObjectKind::Route);
    if !store.entries_ok() {
        return Err(Error::Store(StoreError::Media));
    }
    if !route {
        return Ok(());
    }
    let checkpoint = read_checkpoint_value(store)?;
    if checkpoint.is_some_and(|c| c.route.object == id.0 || c.original.is_some_and(|o| o.object == id.0)) {
        return Err(Error::Store(StoreError::Busy));
    }
    Ok(())
}

fn source_head<D: BlockDevice>(store: &FlatStore<D>, id: ObjectId, kind: ObjectKind) -> Result<EntryMeta, Error> {
    let entry = store.entries().find(|entry| entry.id == id && entry.kind == kind && entry.flags.is_route_head());
    if !store.entries_ok() {
        return Err(Error::Store(StoreError::Media));
    }
    entry.ok_or(Error::Stale)
}

pub fn check_scope<D: BlockDevice>(store: &FlatStore<D>, expected: StoreId, sequence: u64) -> Result<(), Error> {
    if store.store_id() != expected {
        return Err(Error::WrongStore);
    }
    if store.mode() == super::Mode::RemountRequired {
        return Err(Error::RemountRequired);
    }
    if store.sequence() != sequence {
        return Err(Error::Stale);
    }
    Ok(())
}

// Keep the full metadata image out of the storage dispatcher frame.
#[inline(never)]
pub(crate) fn protected_routes<D: BlockDevice>(store: &FlatStore<D>) -> Result<[Option<ObjectId>; 2], Error> {
    Ok(read_checkpoint_value(store)?
        .map_or([None, None], |c| [Some(ObjectId(c.route.object)), c.original.map(|o| ObjectId(o.object))]))
}
