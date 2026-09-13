//! Card-local retention metadata. The caller supplies the payload workspace.
//! This substrate does not admit archive receipts or run retention policy.

use super::{
    BlockDevice, EntryFlags, EntryMeta, FlatStore, Mutation, ObjectId, ObjectKind, PutSource, Revision, Store,
    StoreError, StoreId,
};

pub const HEADER_LEN: usize = 32;
pub const ROW_LEN: usize = 40;
pub const MAX_ROUTES: usize = 64;
pub const MAX_RIDES: usize = 128;
pub const MAX_LEN: usize = HEADER_LEN + (MAX_ROUTES + MAX_RIDES) * ROW_LEN;

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

/// A route use stamp or a finalized ride archive stamp, bound to exact source bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub id: ObjectId,
    pub revision: Revision,
    pub payload_len: u64,
    pub payload_crc: u32,
    pub timestamp: u32,
    pub kind: ObjectKind,
    pub retention: u8,
}

impl Row {
    fn valid(self) -> bool {
        self.id.0 != 0
            && self.revision.0 != 0
            && match self.kind {
                ObjectKind::Route => self.retention <= 5,
                ObjectKind::Ride => self.retention == 0,
                _ => false,
            }
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
        let row = Self {
            id: ObjectId(u64::from_le_bytes(bytes[0..8].try_into().unwrap())),
            revision: Revision(u64::from_le_bytes(bytes[8..16].try_into().unwrap())),
            payload_len: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            payload_crc: u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            timestamp: u32::from_le_bytes(bytes[28..32].try_into().unwrap()),
            kind: ObjectKind::decode(u16::from_le_bytes(bytes[32..34].try_into().unwrap()))
                .map_err(|_| Error::Invalid)?,
            retention: bytes[34],
        };
        if !row.valid() || bytes[35..].iter().any(|&v| v != 0) {
            return Err(Error::Invalid);
        }
        Ok(row)
    }

    fn encode(self, bytes: &mut [u8]) {
        bytes.fill(0);
        bytes[0..8].copy_from_slice(&self.id.0.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.revision.0.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.payload_len.to_le_bytes());
        bytes[24..28].copy_from_slice(&self.payload_crc.to_le_bytes());
        bytes[28..32].copy_from_slice(&self.timestamp.to_le_bytes());
        bytes[32..34].copy_from_slice(&(self.kind as u16).to_le_bytes());
        bytes[34] = self.retention;
    }
}

/// An editable payload image, not evidence that edits are durable.
pub struct Image<'a> {
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
        buffer[4..6].copy_from_slice(&1u16.to_le_bytes());
        buffer[6..8].copy_from_slice(&(HEADER_LEN as u16).to_le_bytes());
        buffer[8..10].copy_from_slice(&(ROW_LEN as u16).to_le_bytes());
        buffer[16..32].copy_from_slice(&store.0);
        Ok(Self { buffer, len: HEADER_LEN, base: None })
    }

    pub fn decode(buffer: &'a mut [u8], len: usize) -> Result<Self, Error> {
        if len < HEADER_LEN || len > buffer.len() || len > MAX_LEN {
            return Err(Error::Invalid);
        }
        let count = u16::from_le_bytes(buffer[10..12].try_into().unwrap()) as usize;
        if &buffer[..4] != b"OBRM"
            || buffer[4..10] != [1, 0, 32, 0, 40, 0]
            || buffer[12..16] != [0; 4]
            || len != HEADER_LEN + count * ROW_LEN
        {
            return Err(Error::Invalid);
        }
        let mut previous = ObjectId::NONE;
        let (mut routes, mut rides) = (0, 0);
        for bytes in buffer[HEADER_LEN..len].as_chunks::<ROW_LEN>().0 {
            let row = Row::decode(bytes)?;
            if row.id <= previous {
                return Err(Error::Invalid);
            }
            previous = row.id;
            routes += usize::from(row.kind == ObjectKind::Route);
            rides += usize::from(row.kind == ObjectKind::Ride);
        }
        if routes > MAX_ROUTES || rides > MAX_RIDES {
            return Err(Error::Capacity);
        }
        Ok(Self { buffer, len, base: None })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.buffer[..self.len]
    }
    pub fn store_id(&self) -> StoreId {
        StoreId(self.buffer[16..32].try_into().unwrap())
    }
    pub fn rows(&self) -> impl Iterator<Item = Row> + '_ {
        self.bytes()[HEADER_LEN..].as_chunks::<ROW_LEN>().0.iter().map(|b| Row::decode(b).unwrap())
    }

    pub fn set(&mut self, row: Row) -> Result<(), Error> {
        if !row.valid() {
            return Err(Error::Invalid);
        }
        let index = self.rows().position(|r| r.id >= row.id).unwrap_or((self.len - HEADER_LEN) / ROW_LEN);
        let at = HEADER_LEN + index * ROW_LEN;
        let existing = self.rows().nth(index).filter(|r| r.id == row.id);
        let count = self.rows().filter(|r| r.kind == row.kind).count()
            + usize::from(existing.is_none_or(|old| old.kind != row.kind));
        let capacity = if row.kind == ObjectKind::Route { MAX_ROUTES } else { MAX_RIDES };
        if count > capacity || (existing.is_none() && self.len + ROW_LEN > self.buffer.len()) {
            return Err(Error::Capacity);
        }
        if existing.is_none() {
            self.buffer.copy_within(at..self.len, at + ROW_LEN);
            self.len += ROW_LEN;
            self.update_count();
        }
        row.encode(&mut self.buffer[at..at + ROW_LEN]);
        Ok(())
    }

    fn update_count(&mut self) {
        self.buffer[10..12].copy_from_slice(&(((self.len - HEADER_LEN) / ROW_LEN) as u16).to_le_bytes());
    }

    /// Remove stale rows only after a complete, successful catalog scan.
    pub fn reconcile<D: BlockDevice>(&mut self, store: &FlatStore<D>) -> Result<bool, Error> {
        if self.store_id() != store.store_id() {
            return Err(Error::WrongStore);
        }
        let mut keep = [false; MAX_ROUTES + MAX_RIDES];
        for entry in store.entries() {
            for (index, row) in self.rows().enumerate() {
                keep[index] |= row.matches(entry);
            }
        }
        if !store.entries_ok() {
            return Err(Error::Store(StoreError::Media));
        }
        let old_len = self.len;
        let mut dest = HEADER_LEN;
        for (index, &keep) in keep.iter().take((old_len - HEADER_LEN) / ROW_LEN).enumerate() {
            if keep {
                let at = HEADER_LEN + index * ROW_LEN;
                self.buffer.copy_within(at..at + ROW_LEN, dest);
                dest += ROW_LEN;
            }
        }
        self.len = dest;
        self.update_count();
        Ok(self.len != old_len)
    }
}

/// One metadata writer per mounted card. A commit error latches this owner until remount.
/// The runtime must also stop other writers when it receives `RemountRequired`.
pub struct Metadata {
    store: StoreId,
    head: Option<EntryMeta>,
    loaded: bool,
    blocked: bool,
}

impl Metadata {
    pub fn new<D: BlockDevice>(store: &FlatStore<D>) -> Self {
        Self { store: store.store_id(), head: None, loaded: false, blocked: false }
    }

    fn check<D: BlockDevice>(&self, store: &FlatStore<D>) -> Result<(), Error> {
        if self.blocked {
            return Err(Error::RemountRequired);
        }
        if self.store != store.store_id() {
            return Err(Error::WrongStore);
        }
        Ok(())
    }

    pub fn load<'a, D: BlockDevice>(&mut self, store: &FlatStore<D>, buffer: &'a mut [u8]) -> Result<Image<'a>, Error> {
        self.check(store)?;
        self.loaded = false;
        let head = singleton(store)?;
        let mut image = if let Some(head) = head {
            let len = usize::try_from(head.payload_len).map_err(|_| Error::Capacity)?;
            if len > buffer.len() || len > MAX_LEN {
                return Err(Error::Capacity);
            }
            read_payload(store, head, &mut buffer[..len])?;
            Image::decode(buffer, len)?
        } else {
            Image::empty(self.store, buffer)?
        };
        if image.store_id() != self.store {
            return Err(Error::WrongStore);
        }
        image.base = Some(head);
        self.head = head;
        self.loaded = true;
        Ok(image)
    }

    /// Publish an edited image only while both the metadata head and target source still match.
    /// `Some(target)` is captured before choosing a stamp. `None` permits pruning-only publication,
    /// including an empty image. Every remaining row must still name a current source head.
    pub fn replace<D: BlockDevice>(
        &mut self,
        store: &FlatStore<D>,
        image: &mut Image<'_>,
        target: Option<EntryMeta>,
    ) -> Result<EntryMeta, Error> {
        self.check(store)?;
        if !self.loaded || image.store_id() != self.store {
            return Err(Error::WrongStore);
        }
        if image.base != Some(self.head) || singleton(store)? != self.head {
            return Err(Error::Stale);
        }
        let mut target_present = false;
        let mut found = [false; MAX_ROUTES + MAX_RIDES];
        for entry in store.entries() {
            target_present |= Some(entry) == target && entry.flags == EntryFlags::NONE;
            for (index, row) in image.rows().enumerate() {
                found[index] |= row.matches(entry);
            }
        }
        if !store.entries_ok() {
            return Err(Error::Store(StoreError::Media));
        }
        if found.iter().take(image.rows().count()).any(|&v| !v)
            || target.is_some_and(|target| !target_present || !image.rows().any(|row| row.matches(target)))
        {
            return Err(Error::Stale);
        }
        let (id, revision) = match self.head {
            Some(head) => (head.id, Revision(head.revision.0.checked_add(1).ok_or(Error::Invalid)?)),
            None => (store.next_object_id(), Revision(1)),
        };
        let meta = EntryMeta {
            id,
            revision,
            kind: ObjectKind::Metadata,
            flags: EntryFlags::NONE,
            payload_len: image.len as u64,
            payload_crc: obc_crc::crc32(image.bytes()),
            name: Default::default(),
        };
        let sequence = store.sequence();
        let mut allocation = store.allocate(meta.payload_len)?;
        if let Err(error) = store.write(&mut allocation, image.bytes()) {
            store.cancel(allocation);
            return Err(error.into());
        }
        if store.store_id() != self.store || store.sequence() != sequence {
            store.cancel(allocation);
            return Err(Error::Stale);
        }
        let put = Mutation::Put { meta, source: PutSource::Fresh(allocation) };
        let result = match self.head {
            Some(old) => store.commit(&[Mutation::Remove { id: old.id, revision: old.revision }, put]),
            None => store.commit(&[put]),
        };
        if result.is_err() {
            self.blocked = true; // a new gate may be durable: do not recycle its payload allocation
            return Err(Error::RemountRequired);
        }
        self.head = Some(meta);
        let handle = store.open(id, Some(revision));
        let verified = handle.and_then(|handle| {
            let mut block = [0u8; 512];
            let result = image.bytes().chunks(512).enumerate().try_for_each(|(i, want)| {
                let got = store.read(&handle, (i * 512) as u64, &mut block[..want.len()])?;
                if got != want.len() || &block[..got] != want {
                    return Err(StoreError::Media);
                }
                Ok(())
            });
            store.close(handle);
            result
        });
        if verified.is_err() {
            self.blocked = true;
            return Err(Error::RemountRequired);
        }
        image.base = Some(Some(meta));
        Ok(meta)
    }
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

#[cfg(test)]
mod tests;
