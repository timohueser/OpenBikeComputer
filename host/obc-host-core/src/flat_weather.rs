//! One validated weather bundle on the session card, with revision-pinned readers.
use crate::flat_store::{HostMedia, HostStore, ImportError, ObjectSource};
use obc_formats::io::SliceSource;
use obc_storage::flat::{
    DisplayName, EntryFlags, EntryMeta, FlatStore, ObjectId, ObjectKind, Revision, Store, StoreError, StoreId,
};
use obc_weather::{ValidatedBundle, WeatherReader};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeatherIdentity {
    pub store: StoreId,
    pub id: ObjectId,
    pub revision: Revision,
}
#[derive(Debug)]
pub enum WeatherError {
    Storage(StoreError),
    Import(ImportError),
    Format(obc_weather::Error),
    Ambiguous,
    RemountRequired,
    AwaitingReader(WeatherIdentity),
}
impl std::fmt::Display for WeatherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "weather card: {self:?}")
    }
}
impl From<StoreError> for WeatherError {
    fn from(error: StoreError) -> Self {
        Self::Storage(error)
    }
}
impl From<ImportError> for WeatherError {
    fn from(error: ImportError) -> Self {
        match error {
            ImportError::RemountRequired => Self::RemountRequired,
            error => Self::Import(error),
        }
    }
}
impl From<obc_weather::Error> for WeatherError {
    fn from(error: obc_weather::Error) -> Self {
        Self::Format(error)
    }
}

#[derive(Debug)]
pub enum WeatherInstall {
    Adopted(WeatherIdentity),
    /// The write succeeded. Retry only reader acquisition, never the import.
    AwaitingReader {
        identity: WeatherIdentity,
        error: WeatherError,
    },
}
struct Bundle {
    source: ObjectSource,
    validated: ValidatedBundle,
}
impl Bundle {
    fn identity(&self) -> WeatherIdentity {
        WeatherIdentity { store: self.source.store_id(), id: self.source.id(), revision: self.source.revision() }
    }
}

pub struct FlatWeatherStore {
    owner: HostStore,
    held: Option<Bundle>,
    pending: Option<WeatherIdentity>,
    catalog_valid: bool,
}
impl FlatWeatherStore {
    /// An absent bundle is empty. Unreadable, invalid, or multiple bundles are errors.
    pub fn open(owner: HostStore) -> Result<Self, WeatherError> {
        let mut store = Self { owner, held: None, pending: None, catalog_valid: false };
        store.refresh()?;
        Ok(store)
    }
    fn head(&self) -> Result<Option<(StoreId, EntryMeta)>, WeatherError> {
        let owner = self.owner.0.lock().map_err(|_| WeatherError::RemountRequired)?;
        let store = owner.ready().map_err(|_| WeatherError::RemountRequired)?;
        Self::head_of(store)
    }
    fn head_of(store: &FlatStore<HostMedia>) -> Result<Option<(StoreId, EntryMeta)>, WeatherError> {
        if store.mode() == obc_storage::flat::Mode::RemountRequired {
            return Err(WeatherError::RemountRequired);
        }
        let mut head = None;
        for entry in store.entries().filter(|e| e.kind == ObjectKind::WeatherBundle && e.flags == EntryFlags::NONE) {
            if head.replace(entry).is_some() {
                return Err(WeatherError::Ambiguous);
            }
        }
        if !store.entries_ok() {
            return Err(WeatherError::Storage(StoreError::Media));
        }
        Ok(head.map(|entry| (store.store_id(), entry)))
    }
    /// Reconcile the current head, including an already-committed reader acquisition.
    /// Old leases stay valid. A superseding head is adopted under its own identity.
    pub fn refresh(&mut self) -> Result<bool, WeatherError> {
        self.catalog_valid = false;
        let Some((store, head)) = self.head()? else {
            self.pending = None;
            self.catalog_valid = true;
            return Ok(self.held.take().is_some());
        };
        let identity = WeatherIdentity { store, id: head.id, revision: head.revision };
        if self.identity() == Some(identity) {
            self.pending = None;
            self.catalog_valid = true;
            return Ok(false);
        }
        let source = self.owner.open(head.id, head.revision)?;
        let validated = WeatherReader::open(&source)?.validated();
        // The input source is immutable, but another writer can supersede its head during validation.
        if !source.is_current() || self.head()? != Some((store, head)) {
            return Err(WeatherError::Storage(StoreError::NotFound));
        }
        if self.pending != Some(identity) {
            // A reopened file can expose an unflushed gate through the OS cache. Validation is
            // not a durability barrier. Check this exact singleton and sync under one owner lock.
            let mut owner = self.owner.0.lock().map_err(|_| WeatherError::RemountRequired)?;
            if Self::head_of(owner.ready().map_err(|_| WeatherError::RemountRequired)?)? != Some((store, head)) {
                return Err(WeatherError::Storage(StoreError::NotFound));
            }
            owner.confirm_durable().map_err(|_| WeatherError::RemountRequired)?;
        }
        self.held = Some(Bundle { source, validated });
        self.pending = None;
        self.catalog_valid = true;
        Ok(true)
    }
    pub fn pending(&self) -> Option<WeatherIdentity> {
        self.pending
    }
    /// Identity of the last successfully mounted bundle. It may be a retained display fallback.
    pub fn identity(&self) -> Option<WeatherIdentity> {
        self.held.as_ref().map(Bundle::identity)
    }
    /// Classifiers must use current card data, never a retained display fallback.
    pub fn current_header(&self) -> Option<obc_formats::obcw::Header> {
        let held = self.held.as_ref()?;
        (self.catalog_valid && self.pending.is_none() && held.source.is_current()).then(|| held.validated.header())
    }
    pub fn reader(&self) -> Result<Option<WeatherReader<'_, ObjectSource>>, WeatherError> {
        self.held.as_ref().map(|held| held.validated.reader(&held.source).map_err(WeatherError::from)).transpose()
    }
    pub fn source(&self) -> Option<ObjectSource> {
        self.held.as_ref().map(|held| held.source.clone())
    }

    /// Validate immutable input before an atomic replacement. Leave the held bundle unchanged on
    /// precommit failure; keep a successful commit's exact identity if its reader is unavailable.
    pub fn install(&mut self, bytes: &[u8]) -> Result<WeatherInstall, WeatherError> {
        if let Some(identity) = self.pending {
            return Err(WeatherError::AwaitingReader(identity));
        }
        WeatherReader::open(&SliceSource(bytes))?;
        let expected = self.identity().map(|identity| (identity.id, identity.revision));
        let store = self.owner.store_id()?;
        let meta = self.owner.import(
            ObjectKind::WeatherBundle,
            expected,
            &mut &bytes[..],
            bytes.len() as u64,
            DisplayName::default(),
        )?;
        let identity = WeatherIdentity { store, id: meta.id, revision: meta.revision };
        self.pending = Some(identity);
        match self.refresh() {
            Ok(_) if self.identity() == Some(identity) => Ok(WeatherInstall::Adopted(identity)),
            Ok(_) => {
                Ok(WeatherInstall::AwaitingReader { identity, error: WeatherError::Storage(StoreError::NotFound) })
            }
            Err(error) => Ok(WeatherInstall::AwaitingReader { identity, error }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::io::ByteSource;
    const WEATHER: &[u8] = include_bytes!("../../../specs/vectors/weather-minimal-dry.obcw");

    fn bytes(source: &ObjectSource) -> Vec<u8> {
        let mut bytes = vec![0; source.len() as usize];
        source.read_at(0, &mut bytes).unwrap();
        bytes
    }

    #[test]
    fn replacement_pins_old_bytes_and_refuses_stale_or_invalid_input() {
        let owner = HostStore::memory().unwrap();
        let mut weather = FlatWeatherStore::open(owner.clone()).unwrap();
        assert!(weather.identity().is_none());
        weather.install(WEATHER).unwrap();
        let old = weather.source().unwrap();
        let identity = weather.identity().unwrap();
        let mut stale = FlatWeatherStore::open(owner.clone()).unwrap();
        let sequence = owner.0.lock().unwrap().card.sequence();
        assert!(matches!(weather.install(&WEATHER[..511]), Err(WeatherError::Format(_))));
        assert_eq!(owner.0.lock().unwrap().card.sequence(), sequence);
        let (a, b) = {
            let state = owner.0.lock().unwrap();
            (state.card.allocate(512).unwrap(), state.card.allocate(512).unwrap())
        };
        assert!(matches!(weather.install(WEATHER), Err(WeatherError::Import(ImportError::Storage(_)))));
        assert_eq!(weather.identity(), Some(identity));
        assert!(weather.current_header().is_some());
        {
            let state = owner.0.lock().unwrap();
            state.card.cancel(a);
            state.card.cancel(b);
        }
        let mut input = WEATHER.to_vec();
        weather.install(&input).unwrap();
        input.fill(0);
        assert_eq!(bytes(&old), WEATHER);
        assert!(!old.is_current());
        assert_eq!(bytes(&weather.source().unwrap()), WEATHER);
        assert_eq!(weather.identity().unwrap(), WeatherIdentity { revision: Revision(2), ..identity });
        assert!(matches!(
            stale.install(WEATHER),
            Err(WeatherError::Import(ImportError::Storage(StoreError::NotFound)))
        ));
        assert!(stale.current_header().is_none());
        stale.refresh().unwrap();
        assert_eq!(stale.identity(), weather.identity());
        let reader = weather.reader().unwrap().unwrap();
        let expected = WeatherReader::open(&SliceSource(WEATHER)).unwrap();
        assert_eq!(reader.header(), expected.header());
        assert_eq!(reader.hourly_records().unwrap(), expected.hourly_records().unwrap());
    }

    #[test]
    fn reader_pressure_keeps_commit_identity_without_reimport_and_reconciles_superseding_head() {
        let owner = HostStore::memory().unwrap();
        let mut holds = Vec::new();
        for _ in 0..5 {
            let entry =
                owner.import(ObjectKind::MapShard, None, &mut &b"other"[..], 5, DisplayName::default()).unwrap();
            holds.push(owner.open(entry.id, entry.revision).unwrap());
        }
        let mut weather = FlatWeatherStore::open(owner.clone()).unwrap();
        weather.install(WEATHER).unwrap();
        let old = weather.source().unwrap();
        let WeatherInstall::AwaitingReader { identity, error: WeatherError::Storage(StoreError::Busy) } =
            weather.install(WEATHER).unwrap()
        else {
            panic!("expected committed head without reader")
        };
        assert_eq!(identity.revision, Revision(2));
        let committed_sequence = owner.0.lock().unwrap().card.sequence();
        assert!(matches!(weather.install(WEATHER), Err(WeatherError::AwaitingReader(id)) if id == identity));
        assert!(weather.current_header().is_none());
        assert_eq!(bytes(&old), WEATHER);
        assert!(matches!(weather.refresh(), Err(WeatherError::Storage(StoreError::Busy))));
        holds.pop();
        weather.refresh().unwrap();
        assert_eq!(weather.identity(), Some(identity));
        assert_eq!(owner.0.lock().unwrap().card.sequence(), committed_sequence);
        drop(old);
        let mut other = FlatWeatherStore::open(owner.clone()).unwrap();
        // Two owners share the current hold row; fill the spare row before replacement.
        let entry = owner.import(ObjectKind::MapShard, None, &mut &b"other"[..], 5, DisplayName::default()).unwrap();
        holds.push(owner.open(entry.id, entry.revision).unwrap());
        let WeatherInstall::AwaitingReader { identity: next, .. } = other.install(WEATHER).unwrap() else {
            panic!("reader table remains full")
        };
        owner.remove(ObjectKind::WeatherBundle, next.id, next.revision).unwrap();
        other.refresh().unwrap();
        assert!(other.identity().is_none());
        assert!(other.pending().is_none());
        assert!(weather.current_header().is_none());
    }

    #[test]
    fn distinct_weather_heads_are_ambiguous_even_when_both_validate() {
        use obc_storage::flat::{Mutation, PutSource};
        let owner = HostStore::memory().unwrap();
        let mut weather = FlatWeatherStore::open(owner.clone()).unwrap();
        weather.install(WEATHER).unwrap();
        // A raw card producer can publish another ID; the runtime must not pick one arbitrarily.
        let state = owner.0.lock().unwrap();
        let mut allocation = state.card.allocate(WEATHER.len() as u64).unwrap();
        state.card.write(&mut allocation, WEATHER).unwrap();
        let meta = EntryMeta {
            id: state.card.next_object_id(),
            revision: Revision(1),
            kind: ObjectKind::WeatherBundle,
            flags: EntryFlags::NONE,
            payload_len: WEATHER.len() as u64,
            payload_crc: obc_crc::crc32(WEATHER),
            name: DisplayName::default(),
        };
        state.card.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]).unwrap();
        drop(state);
        assert!(matches!(weather.refresh(), Err(WeatherError::Ambiguous)));
    }

    #[test]
    fn malformed_catalog_bundle_is_not_an_empty_store() {
        let owner = HostStore::memory().unwrap();
        owner.import(ObjectKind::WeatherBundle, None, &mut &b"invalid"[..], 7, DisplayName::default()).unwrap();
        assert!(matches!(FlatWeatherStore::open(owner), Err(WeatherError::Format(_))));
    }
}
