//! Parsed map tables and cache over a shared flat-store object.

use crate::flat_store::{HostStore, ImportError, ObjectSource};
use obc_reader::{MapCache, MapTables, Reader};
use obc_storage::flat::{DisplayName, ObjectKind, StoreError};
use std::io;

#[derive(Debug)]
pub enum MapError {
    Io(io::Error),
    Storage(StoreError),
    Format(obc_reader::Error),
}
impl From<ImportError> for MapError {
    fn from(error: ImportError) -> Self {
        match error {
            ImportError::Io(e) => Self::Io(e),
            ImportError::Storage(e) => Self::Storage(e),
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
        let source = store.open(meta.id, meta.revision)?;
        Ok(Self { source, tables, cache: MapCache::new_boxed() })
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_file(mut input: std::fs::File) -> Result<Self, MapError> {
        use std::io::Seek;
        input.rewind().map_err(MapError::Io)?;
        let len = input.metadata().map_err(MapError::Io)?.len();
        Self::import(&HostStore::temporary()?, &mut input, len)
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn import(store: &HostStore, input: &mut impl io::Read, len: u64) -> Result<Self, MapError> {
        let meta = store.import(ObjectKind::MapShard, None, input, len, DisplayName::default())?;
        let source = store.open(meta.id, meta.revision)?;
        let tables = MapTables::parse(&source).map_err(MapError::Format)?;
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

#[cfg(test)]
mod tests;
