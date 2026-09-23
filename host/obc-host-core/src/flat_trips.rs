//! Validated trip projections from the same card as their routes.

use crate::{
    flat_routes::catalog_error,
    flat_store::{HostStore, ImportError},
    TripCatalog,
};
use obc_app::{
    catalog_state::CatalogError, metadata::MetadataError, trip::TripProgress, App, CatalogObjectId, TripInput,
};
use obc_formats::io::SliceSource;
use obc_route::TripMeta;
use obc_storage::flat::{metadata, DisplayName, ObjectId, ObjectKind, Revision, StoreError};

pub struct FlatTripStore {
    owner: HostStore,
    rows: Vec<(CatalogObjectId, Revision, TripMeta)>,
    progress: Vec<TripProgress>,
}

impl FlatTripStore {
    pub fn new(owner: HostStore) -> Result<Self, CatalogError> {
        let mut store = Self { owner, rows: Vec::new(), progress: Vec::new() };
        store.rescan()?;
        Ok(store)
    }

    pub fn inputs(&self) -> Vec<TripInput<'_>> {
        self.rows
            .iter()
            .map(|(id, _, trip)| TripInput {
                id: *id,
                key: trip.key,
                name: trip.name.as_str(),
                start_date: trip.start_date,
                stage_ids: trip.day_routes.as_slice(),
            })
            .collect()
    }

    /// The trip progress records as the last rescan read them.
    pub fn progress(&self) -> &[TripProgress] {
        &self.progress
    }

    pub fn import(&mut self, bytes: &[u8]) -> Result<CatalogObjectId, ImportError> {
        let trip = TripMeta::read(&SliceSource(bytes)).map_err(|_| StoreError::Invalid)?;
        if trip.truncated || self.rows.len() >= obc_app::MAX_TRIPS {
            return Err(StoreError::Invalid.into());
        }
        let meta =
            self.owner.import(ObjectKind::Trip, None, &mut &bytes[..], bytes.len() as u64, DisplayName::default())?;
        self.rows.push((meta.id.0, meta.revision, trip));
        Ok(meta.id.0)
    }
}

impl TripCatalog for FlatTripStore {
    fn store_scope(&self) -> Option<obc_app::device_core::StoreRevision> {
        let owner = self.owner.0.lock().ok()?;
        Some(crate::flat_routes::scope(owner.ready().ok()?))
    }

    fn delete_by_id(&mut self, id: CatalogObjectId) -> Result<bool, CatalogError> {
        let entries = self.owner.entries().map_err(|error| catalog_error(&self.owner, error))?;
        let Some(meta) = entries.into_iter().find(|entry| entry.id.0 == id) else { return Ok(false) };
        if meta.kind != ObjectKind::Trip {
            return Err(CatalogError::Unsupported);
        }
        let (_, revision, _) = self.rows.iter().find(|row| row.0 == id).ok_or(CatalogError::Stale)?;
        match self.owner.remove(ObjectKind::Trip, ObjectId(id), *revision) {
            Ok(()) => {
                self.rows.retain(|row| row.0 != id);
                Ok(true)
            }
            Err(StoreError::NotFound) => Ok(false),
            Err(error) => Err(catalog_error(&self.owner, error)),
        }
    }

    fn rescan(&mut self) -> Result<(), CatalogError> {
        use obc_storage::flat::{EntryFlags, Store};
        let owner = self.owner.0.lock().map_err(|_| CatalogError::Unreadable)?;
        let store = owner.ready().map_err(|_| CatalogError::RemountRequired)?;
        if store.mode() == obc_storage::flat::Mode::RemountRequired {
            return Err(CatalogError::RemountRequired);
        }
        let mut rows = Vec::new();
        for meta in store.entries().filter(|meta| meta.kind == ObjectKind::Trip && meta.flags == EntryFlags::NONE) {
            if rows.len() >= obc_app::MAX_TRIPS {
                return Err(CatalogError::Unreadable);
            }
            let trip = store
                .with_source(meta.id, Some(meta.revision), |source| TripMeta::read(source))
                .map_err(|_| CatalogError::Unreadable)?
                .map_err(|_| CatalogError::Unreadable)?;
            if trip.truncated {
                return Err(CatalogError::Unreadable);
            }
            rows.push((meta.id.0, meta.revision, trip));
        }
        if !store.entries_ok() {
            return Err(CatalogError::Unreadable);
        }
        let mut progress = Vec::new();
        metadata::read_progress(store, |record| progress.push(record)).map_err(|_| CatalogError::Unreadable)?;
        self.rows = rows;
        self.progress = progress;
        Ok(())
    }

    fn refeed(&self, app: &mut App) {
        app.set_trips(&self.inputs());
        app.set_trip_progress(self.progress.iter().cloned());
    }

    fn write_progress(&mut self, record: TripProgress, keys: &[u64]) -> Result<(), MetadataError> {
        let owner = self.owner.0.lock().map_err(|_| MetadataError::WriteFailed)?;
        let store = owner.ready().map_err(|_| MetadataError::RemountRequired)?;
        metadata::write_progress(store, record, |key| keys.contains(&key)).map_err(crate::flat_routes::metadata_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FlatRouteStore, RouteRepository, VecSink};
    const ROUTE: &[u8] = include_bytes!("../../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr");

    fn trip(ids: &[u64]) -> Vec<u8> {
        let mut bytes = VecSink::default();
        let days: Vec<_> = ids.iter().copied().map(obc_route::TripDay::whole).collect();
        obc_route::write_trip(1, "Stages", 0, &days, &mut bytes).unwrap();
        bytes.bytes().to_vec()
    }

    #[test]
    fn replacement_and_failed_refresh_preserve_exact_trip_authority() {
        let owner = HostStore::memory().unwrap();
        let routes = FlatRouteStore::new(owner.clone(), &[ROUTE]).unwrap();
        let route = routes.ids()[0];
        let mut trips = FlatTripStore::new(owner.clone()).unwrap();
        let id = trips.import(&trip(&[route, 0])).unwrap();
        assert_eq!(trips.delete_by_id(route), Err(CatalogError::Unsupported));
        assert_eq!(routes.ids(), &[route]);
        let old = owner.open(ObjectId(id), Revision(1)).unwrap();
        let bytes = trip(&[route]);
        owner
            .import(
                ObjectKind::Trip,
                Some((ObjectId(id), Revision(1))),
                &mut &bytes[..],
                bytes.len() as u64,
                DisplayName::default(),
            )
            .unwrap();
        assert_eq!(trips.delete_by_id(id), Err(CatalogError::Stale));
        trips.rescan().unwrap();
        assert_eq!(trips.inputs()[0].stage_ids, &[route]);
        assert!(TripMeta::read(&old).is_ok());
        owner.import(ObjectKind::Trip, None, &mut &b"bad"[..], 3, DisplayName::default()).unwrap();
        assert_eq!(trips.rescan(), Err(CatalogError::Unreadable));
        assert_eq!(trips.inputs()[0].id, id, "failure does not replace the last complete projection");
    }
}
