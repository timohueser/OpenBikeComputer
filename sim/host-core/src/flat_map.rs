//! Parsed map tables and cache over a shared flat-store object.

use crate::flat_store::{HostStore, ImportError, ObjectSource};
use obc_reader::{MapCache, MapTables, Reader};
use obc_storage::flat::{DisplayName, EntryMeta, Mode, ObjectId, ObjectKind, Revision, StoreError, StoreId};
use std::io;

#[derive(Debug)]
pub enum MapError {
    Io(io::Error),
    Storage(StoreError),
    Format(obc_reader::Error),
    Mount(Mode),
    RemountRequired,
    CommittedWithoutReader { store_id: StoreId, id: ObjectId, revision: Revision, error: StoreError },
}
impl From<ImportError> for MapError {
    fn from(error: ImportError) -> Self {
        match error {
            ImportError::Io(e) => Self::Io(e),
            ImportError::Storage(e) => Self::Storage(e),
            ImportError::Mount(mode) => Self::Mount(mode),
            ImportError::RemountRequired => Self::RemountRequired,
        }
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
            Self::Io(e) => write!(f, "map import: {e}"),
            Self::Storage(e) => write!(f, "map storage: {e:?}"),
            Self::Format(e) => write!(f, "invalid OBCM file: {e:?}"),
            Self::Mount(mode) => write!(f, "host card cannot be mounted: {mode:?}"),
            Self::RemountRequired => write!(f, "host card commit is uncertain; close all readers and reopen"),
            Self::CommittedWithoutReader { store_id, id, revision, error } => {
                write!(f, "map committed at {store_id:?}/{id:?}/{revision:?}, but reader open failed: {error:?}")
            }
        }
    }
}

pub struct FlatMap {
    source: ObjectSource,
    tables: MapTables,
    cache: Box<MapCache>,
}
impl FlatMap {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, MapError> {
        Self::from_bytes_in(&HostStore::memory()?, bytes)
    }
    pub fn from_bytes_in(store: &HostStore, bytes: &[u8]) -> Result<Self, MapError> {
        let tables = MapTables::parse(&obc_formats::io::SliceSource(bytes)).map_err(MapError::Format)?;
        let meta =
            store.import(ObjectKind::MapShard, None, &mut &bytes[..], bytes.len() as u64, DisplayName::default())?;
        Self::committed(store, meta, tables)
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_file(input: std::fs::File) -> Result<Self, MapError> {
        Self::from_file_in(&HostStore::temporary()?, input)
    }

    /// Import into the same card owner as routes and other native objects.
    /// Validate a private temporary copy so external edits cannot change the parsed bytes.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_file_in(store: &HostStore, input: std::fs::File) -> Result<Self, MapError> {
        Self::import_file(store, input, None)
    }

    /// Replace this exact map revision. The old reader stays pinned on success or failure.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn replace_from_file(&self, input: std::fs::File) -> Result<Self, MapError> {
        Self::import_file(
            &HostStore(self.source.0.owner.clone()),
            input,
            Some((self.source.id(), self.source.revision())),
        )
    }

    /// Reopen the only live map on a card. Ambiguity is an error, never an arbitrary selection.
    pub fn open_only_in(store: &HostStore) -> Result<Self, MapError> {
        let mut maps = store.entries()?.into_iter().filter(|entry| entry.kind == ObjectKind::MapShard);
        let map = maps.next().ok_or(StoreError::NotFound)?;
        if maps.next().is_some() {
            return Err(StoreError::Invalid.into());
        }
        Self::open_in(store, store.store_id()?, map.id, map.revision)
    }

    /// Reopen one persisted map identity; another card with the same numeric id is refused.
    pub fn open_in(store: &HostStore, store_id: StoreId, id: ObjectId, revision: Revision) -> Result<Self, MapError> {
        if store.store_id()? != store_id {
            return Err(StoreError::Invalid.into());
        }
        let entry = store
            .entries()?
            .into_iter()
            .find(|entry| entry.id == id && entry.revision == revision)
            .ok_or(StoreError::NotFound)?;
        if entry.kind != ObjectKind::MapShard {
            return Err(StoreError::Invalid.into());
        }
        let source = store.open(id, revision)?;
        let tables = MapTables::parse(&source).map_err(MapError::Format)?;
        Ok(Self { source, tables, cache: MapCache::new_boxed() })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn import_file(
        store: &HostStore,
        input: std::fs::File,
        previous: Option<(ObjectId, Revision)>,
    ) -> Result<Self, MapError> {
        use obc_formats::io::ByteSource;
        use std::io::Seek;
        let input = snapshot(input)?;
        let len = input.len();
        let tables = MapTables::parse(&input).map_err(MapError::Format)?;
        let mut file = input.into_file();
        file.rewind().map_err(MapError::Io)?;
        let meta = store.import(ObjectKind::MapShard, previous, &mut file, len, DisplayName::default())?;
        Self::committed(store, meta, tables)
    }
    fn committed(store: &HostStore, meta: EntryMeta, tables: MapTables) -> Result<Self, MapError> {
        let store_id = store.store_id()?;
        let source = store.open(meta.id, meta.revision).map_err(|error| MapError::CommittedWithoutReader {
            store_id,
            id: meta.id,
            revision: meta.revision,
            error,
        })?;
        Ok(Self { source, tables, cache: MapCache::new_boxed() })
    }

    pub fn source(&self) -> ObjectSource {
        self.source.clone()
    }
    pub fn tables(&self) -> &MapTables {
        &self.tables
    }
    pub fn reader(&self) -> Reader<'_> {
        Reader::new(&self.source, &self.tables, &self.cache)
    }
}

/// `input` copied into a temporary file. The map is parsed and published from this private copy,
/// so a later edit to the caller's file cannot change the tables it already stated.
#[cfg(not(target_arch = "wasm32"))]
fn snapshot(mut input: std::fs::File) -> Result<obc_file_source::FileSource, MapError> {
    use std::io::{Read, Seek, Write};
    let len = input.metadata().map_err(MapError::Io)?.len();
    input.rewind().map_err(MapError::Io)?;
    let mut file = tempfile::tempfile().map_err(MapError::Io)?;
    let mut buffer = [0; crate::flat_store::IMPORT_BUFFER_BYTES];
    let mut remaining = len;
    while remaining != 0 {
        let take = remaining.min(buffer.len() as u64) as usize;
        input.read_exact(&mut buffer[..take]).map_err(MapError::Io)?;
        file.write_all(&buffer[..take]).map_err(MapError::Io)?;
        remaining -= take as u64;
    }
    if input.read(&mut buffer[..1]).map_err(MapError::Io)? != 0 {
        return Err(MapError::Io(io::ErrorKind::InvalidData.into()));
    }
    obc_file_source::FileSource::from_file(file).map_err(MapError::Io)
}

#[cfg(test)]
mod tests;
