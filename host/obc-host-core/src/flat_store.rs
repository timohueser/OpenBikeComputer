//! Shared session card and revision-pinned sources for native and browser hosts.

use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::{self, Read},
    sync::{Arc, Mutex},
};

use obc_formats::io::{ByteSource, Error};
use obc_storage::flat::{
    BlockDevice, DisplayName, EntryFlags, EntryMeta, FlatStore, Handle, Mutation, ObjectId, ObjectKind, PutSource,
    Revision, Store, StoreError, StoreId,
};

const BLOCK: u64 = 512;
pub(crate) const PAGE: usize = 16 * 1024;
const CARD_BYTES: u64 = 32 * 1024 * 1024 * 1024;
/// Maximum input buffer used while importing a native map.
pub const IMPORT_BUFFER_BYTES: usize = 16 * 1024;

#[derive(Debug)]
pub enum ImportError {
    Io(io::Error),
    Storage(StoreError),
}

impl From<io::Error> for ImportError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<StoreError> for ImportError {
    fn from(error: StoreError) -> Self {
        Self::Storage(error)
    }
}
impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "object import: {error}"),
            Self::Storage(error) => write!(f, "object storage: {error:?}"),
        }
    }
}

/// The backing storage is private: callers read the committed object, never raw card offsets.
pub(crate) enum HostMedia {
    Memory(RefCell<BTreeMap<u64, Box<[u8; PAGE]>>>),
    #[cfg(not(target_arch = "wasm32"))]
    File(RefCell<tempfile::NamedTempFile>),
}

impl HostMedia {
    fn range(lba: u64, len: usize) -> io::Result<u64> {
        let offset = lba.checked_mul(BLOCK).ok_or(io::ErrorKind::InvalidInput)?;
        if !len.is_multiple_of(BLOCK as usize) || offset.checked_add(len as u64).is_none_or(|end| end > CARD_BYTES) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(offset)
    }
}

impl BlockDevice for HostMedia {
    type Error = io::Error;
    fn block_count(&self) -> io::Result<u64> {
        Ok(CARD_BYTES / BLOCK)
    }

    fn read(&self, lba: u64, buf: &mut [u8]) -> io::Result<()> {
        let offset = Self::range(lba, buf.len())?;
        match self {
            Self::Memory(pages) => {
                let pages = pages.borrow();
                let mut done = 0;
                while done < buf.len() {
                    let at = offset + done as u64;
                    let within = (at % PAGE as u64) as usize;
                    let take = (PAGE - within).min(buf.len() - done);
                    let out = &mut buf[done..done + take];
                    if let Some(page) = pages.get(&(at / PAGE as u64)) {
                        out.copy_from_slice(&page[within..within + take]);
                    } else {
                        out.fill(0);
                    }
                    done += take;
                }
                Ok(())
            }
            #[cfg(not(target_arch = "wasm32"))]
            Self::File(file) => {
                use std::io::{Seek, SeekFrom};
                let mut file = file.borrow_mut();
                file.seek(SeekFrom::Start(offset))?;
                file.read_exact(buf)
            }
        }
    }

    fn write(&self, lba: u64, buf: &[u8]) -> io::Result<()> {
        let offset = Self::range(lba, buf.len())?;
        match self {
            Self::Memory(pages) => {
                let mut pages = pages.borrow_mut();
                let mut done = 0;
                while done < buf.len() {
                    let at = offset + done as u64;
                    let within = (at % PAGE as u64) as usize;
                    let take = (PAGE - within).min(buf.len() - done);
                    let page = pages.entry(at / PAGE as u64).or_insert_with(|| Box::new([0; PAGE]));
                    page[within..within + take].copy_from_slice(&buf[done..done + take]);
                    done += take;
                }
                Ok(())
            }
            #[cfg(not(target_arch = "wasm32"))]
            Self::File(file) => {
                use std::io::{Seek, SeekFrom, Write};
                let mut file = file.borrow_mut();
                file.seek(SeekFrom::Start(offset))?;
                file.write_all(buf)
            }
        }
    }

    fn sync(&self) -> io::Result<()> {
        match self {
            Self::Memory(_) => Ok(()),
            #[cfg(not(target_arch = "wasm32"))]
            Self::File(file) => file.borrow().as_file().sync_all(),
        }
    }
}

type Owner = Arc<Mutex<FlatStore<HostMedia>>>;

pub(crate) struct Lease {
    pub(crate) owner: Owner,
    handle: Option<Handle>,
    len: u64,
    store_id: StoreId,
}

impl Drop for Lease {
    fn drop(&mut self) {
        // A poisoned owner has already failed. Still return its handle during unwind.
        let store = self.owner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        store.close(self.handle.take().expect("the last reader owns the handle"));
    }
}

/// Cloneable object bytes pinned to one object revision. The last clone closes the lease.
#[derive(Clone)]
pub struct ObjectSource(pub(crate) Arc<Lease>);

impl ObjectSource {
    pub(crate) fn open(owner: Owner, id: ObjectId, revision: Option<Revision>) -> Result<Self, StoreError> {
        let store = owner.lock().map_err(|_| StoreError::Media)?;
        let handle = store.open(id, revision)?;
        let len = store.handle_len(&handle).expect("a just-opened handle resolves");
        let store_id = store.store_id();
        drop(store);
        Ok(Self(Arc::new(Lease { owner, handle: Some(handle), len, store_id })))
    }

    pub fn store_id(&self) -> StoreId {
        self.0.store_id
    }

    pub fn id(&self) -> ObjectId {
        self.0.handle.as_ref().unwrap().id()
    }
    pub fn revision(&self) -> Revision {
        self.0.handle.as_ref().unwrap().revision()
    }
}

impl ByteSource for ObjectSource {
    fn len(&self) -> u64 {
        self.0.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        // Match StoreSource: check the range before media, then fill across short reads.
        let end = offset.checked_add(buf.len() as u64).ok_or(Error::BadOffset)?;
        if end > self.0.len {
            return Err(Error::BadOffset);
        }
        if buf.is_empty() {
            return Ok(());
        }
        let store = self.0.owner.lock().map_err(|_| Error::Io)?;
        let handle = self.0.handle.as_ref().unwrap();
        let mut done = 0;
        while done < buf.len() {
            match store.read(handle, offset + done as u64, &mut buf[done..]) {
                Ok(0) | Err(_) => return Err(Error::Io),
                Ok(n) => done += n,
            }
        }
        Ok(())
    }
}

/// One session card. Moving it into the route repository gives that repository sole mutation
/// ownership. Object readers keep the card alive after the front-end owner drops.
pub struct HostStore(pub(crate) Owner);

impl HostStore {
    pub fn memory() -> Result<Self, ImportError> {
        Self::new(HostMedia::Memory(RefCell::default()))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn temporary() -> Result<Self, ImportError> {
        let card = tempfile::NamedTempFile::new()?;
        card.as_file().set_len(CARD_BYTES)?;
        Self::new(HostMedia::File(RefCell::new(card)))
    }

    fn new(media: HostMedia) -> Result<Self, ImportError> {
        let mut identity = [0; 16];
        getrandom::getrandom(&mut identity).map_err(|error| io::Error::other(error.to_string()))?;
        Ok(Self(Arc::new(Mutex::new(FlatStore::initialize(media, StoreId(identity))?))))
    }

    pub fn open(&self, id: ObjectId, revision: Revision) -> Result<ObjectSource, StoreError> {
        ObjectSource::open(self.0.clone(), id, Some(revision))
    }

    pub(crate) fn entries(&self) -> Result<Vec<EntryMeta>, StoreError> {
        let store = self.0.lock().map_err(|_| StoreError::Media)?;
        let entries = store.entries().collect();
        if !store.entries_ok() {
            return Err(StoreError::Media);
        }
        Ok(entries)
    }

    pub(crate) fn remove(&self, kind: ObjectKind, id: ObjectId, revision: Revision) -> Result<(), StoreError> {
        let store = self.0.lock().map_err(|_| StoreError::Media)?;
        let head = store.entries().filter(|entry| entry.id == id).max_by_key(|entry| entry.revision.0);
        if !store.entries_ok() {
            return Err(StoreError::Media);
        }
        let head = head.ok_or(StoreError::NotFound)?;
        if head.kind != kind {
            return Err(StoreError::Invalid);
        }
        if head.revision != revision {
            return Err(StoreError::RevisionConflict { current: head.revision });
        }
        store.commit(&[Mutation::Remove { id, revision }])?;
        Ok(())
    }

    /// Return the committed metadata. Opening a reader is a separate, retryable operation.
    pub(crate) fn import(
        &self,
        kind: ObjectKind,
        previous: Option<(ObjectId, Revision)>,
        input: &mut impl Read,
        len: u64,
        name: DisplayName,
    ) -> Result<EntryMeta, ImportError> {
        let store = self.0.lock().map_err(|_| StoreError::Media)?;
        let id = previous.map_or_else(|| store.next_object_id(), |(id, _)| id);
        let revision = previous.map_or(Ok(Revision(1)), |(_, revision)| {
            revision.0.checked_add(1).map(Revision).ok_or(StoreError::ReadOnly)
        })?;
        let mut allocation = store.allocate(len)?;
        let result = (|| {
            let mut buffer = [0; IMPORT_BUFFER_BYTES];
            let mut crc = obc_crc::Crc32::new();
            let mut remaining = len;
            while remaining != 0 {
                let take = remaining.min(buffer.len() as u64) as usize;
                input.read_exact(&mut buffer[..take])?;
                store.write(&mut allocation, &buffer[..take])?;
                crc.update(&buffer[..take]);
                remaining -= take as u64;
            }
            if input.read(&mut buffer[..1])? != 0 {
                return Err(ImportError::Io(io::ErrorKind::InvalidData.into()));
            }
            let meta = EntryMeta {
                id,
                revision,
                kind,
                flags: EntryFlags::NONE,
                payload_len: len,
                payload_crc: crc.finalize(),
                name,
            };
            let put = Mutation::Put { meta, source: PutSource::Fresh(allocation) };
            if let Some((_, old_revision)) = previous {
                store.commit(&[Mutation::Remove { id, revision: old_revision }, put])?;
            } else {
                store.commit(&[put])?;
            }
            Ok(meta)
        })();
        if result.is_err() {
            store.cancel(allocation);
        }
        result
    }
}
