//! Session-owned maps imported into the same flat store the board reads.
//!
//! Native media is a temporary sparse file; browser media allocates only written pages.
//! The map and a native panorama worker share one pinned lease. A read locks the store for that
//! operation only; parsing, drawing and terrain generation run outside the lock.

use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::{self, Read},
    sync::{Arc, Mutex},
};

use obc_formats::io::{ByteSource, Error};
use obc_reader::{MapCache, MapTables, Reader};
use obc_storage::flat::{
    BlockDevice, DisplayName, EntryFlags, EntryMeta, FlatStore, Handle, Mutation, ObjectId, ObjectKind, PutSource,
    Revision, Store, StoreError, StoreId,
};

const BLOCK: u64 = 512;
const PAGE: usize = 16 * 1024;
const CARD_BYTES: u64 = 32 * 1024 * 1024 * 1024;
/// Maximum input buffer used while importing a native map.
pub const IMPORT_BUFFER_BYTES: usize = 16 * 1024;

#[derive(Debug)]
pub enum MapError {
    Io(io::Error),
    Storage(StoreError),
    Format(obc_reader::Error),
}

impl From<io::Error> for MapError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<StoreError> for MapError {
    fn from(error: StoreError) -> Self {
        Self::Storage(error)
    }
}
impl std::fmt::Display for MapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "map import: {error}"),
            Self::Storage(error) => write!(f, "map storage: {error:?}"),
            Self::Format(error) => write!(f, "invalid OBCM file: {error:?}"),
        }
    }
}

/// The backing storage is private: callers read the committed object, never raw card offsets.
enum HostMedia {
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

struct Lease {
    owner: Owner,
    handle: Option<Handle>,
    len: u64,
}

impl Drop for Lease {
    fn drop(&mut self) {
        // A poisoned owner has already failed. Still return its handle during unwind.
        let store = self.owner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        store.close(self.handle.take().expect("the last reader owns the handle"));
    }
}

/// Cloneable map bytes pinned to one object revision. The last clone closes the lease.
#[derive(Clone)]
pub struct MapSource(Arc<Lease>);

impl MapSource {
    fn open(owner: Owner, id: ObjectId, revision: Option<Revision>) -> Result<Self, StoreError> {
        let store = owner.lock().map_err(|_| StoreError::Media)?;
        let handle = store.open(id, revision)?;
        let len = store.handle_len(&handle).expect("a just-opened handle resolves");
        drop(store);
        Ok(Self(Arc::new(Lease { owner, handle: Some(handle), len })))
    }

    pub fn id(&self) -> ObjectId {
        self.0.handle.as_ref().unwrap().id()
    }
    pub fn revision(&self) -> Revision {
        self.0.handle.as_ref().unwrap().revision()
    }
}

impl ByteSource for MapSource {
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

/// An imported map, its parsed tables and the session cache. Readers borrow these owned inputs.
pub struct FlatMap {
    source: MapSource,
    tables: MapTables,
    cache: Box<MapCache>,
}

impl FlatMap {
    /// Import an embedded map into sparse, volatile browser memory.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, MapError> {
        Self::import(HostMedia::Memory(RefCell::default()), &mut &bytes[..], bytes.len() as u64)
    }

    /// Stream an open OBCM file into a temporary card removed when its last reader drops.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_file(mut input: std::fs::File) -> Result<Self, MapError> {
        use std::io::Seek;
        input.rewind()?;
        let len = input.metadata()?.len();
        let card = tempfile::NamedTempFile::new()?;
        card.as_file().set_len(CARD_BYTES)?;
        Self::import(HostMedia::File(RefCell::new(card)), &mut input, len)
    }

    fn import(media: HostMedia, input: &mut impl Read, len: u64) -> Result<Self, MapError> {
        let mut identity = [0; 16];
        getrandom::getrandom(&mut identity).map_err(|error| io::Error::other(error.to_string()))?;
        let store = FlatStore::initialize(media, StoreId(identity))?;
        let id = store.next_object_id();
        let mut allocation = store.allocate(len)?;
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
        // A changing input must not silently import a prefix under the original length.
        if input.read(&mut buffer[..1])? != 0 {
            return Err(MapError::Io(io::ErrorKind::InvalidData.into()));
        }
        let meta = EntryMeta {
            id,
            revision: Revision(1),
            kind: ObjectKind::MapShard,
            flags: EntryFlags::NONE,
            payload_len: len,
            payload_crc: crc.finalize(),
            name: DisplayName::default(),
        };
        store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }])?;
        let source = MapSource::open(Arc::new(Mutex::new(store)), id, None)?;
        let tables = MapTables::parse(&source).map_err(MapError::Format)?;
        Ok(Self { source, tables, cache: MapCache::new_boxed() })
    }

    pub fn source(&self) -> MapSource {
        self.source.clone()
    }
    pub fn tables(&self) -> &MapTables {
        &self.tables
    }
    pub fn reader(&self) -> Reader<'_> {
        Reader::new(&self.source, &self.tables, &self.cache)
    }
}

#[cfg(test)]
mod tests;
