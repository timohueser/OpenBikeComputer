use crate::{
    flat_routes::{metadata_error, scope},
    flat_store::HostStore,
    RideRepository,
};
use obc_app::{
    catalog_state::CatalogError, device_core::StoreRevision, metadata::MetadataError, CatalogObjectId, RideEntry,
    RideSummary, RideTrip, RideTrips,
};
use obc_route::{Profile, RideInfo};
use obc_storage::flat::{metadata, EntryFlags, ObjectId, ObjectKind, Revision, Store};

pub struct FlatRideStore {
    owner: HostStore,
    catalog: Vec<RideEntry>,
    trips: RideTrips,
    heads: Vec<(ObjectId, Revision)>,
}

impl FlatRideStore {
    pub fn new(owner: HostStore) -> Result<Self, MetadataError> {
        let mut repo = Self { owner, catalog: Vec::new(), trips: RideTrips::new(), heads: Vec::new() };
        repo.refresh_metadata()?;
        Ok(repo)
    }
    /// Import immutable saved-ride bytes as a new card object. Fixture paths are not runtime owners.
    pub fn import(&mut self, bytes: &[u8]) -> Result<obc_app::CatalogObjectId, crate::flat_store::ImportError> {
        let info =
            RideInfo::read(&obc_formats::io::SliceSource(bytes)).map_err(|_| obc_storage::flat::StoreError::Invalid)?;
        let name = obc_storage::flat::DisplayName::new(info.name.as_str()).unwrap_or_default();
        let meta = self.owner.import(ObjectKind::Ride, None, &mut &bytes[..], bytes.len() as u64, name)?;
        self.refresh_metadata().map_err(|_| obc_storage::flat::StoreError::Media)?;
        Ok(meta.id.0)
    }
}

impl RideRepository for FlatRideStore {
    fn catalog(&self) -> &[RideEntry] {
        &self.catalog
    }
    fn trip_names(&self) -> &[RideTrip] {
        &self.trips
    }
    fn store_scope(&self) -> Option<StoreRevision> {
        let owner = self.owner.0.lock().ok()?;
        Some(scope(owner.ready().ok()?))
    }

    fn refresh_metadata(&mut self) -> Result<Option<StoreRevision>, MetadataError> {
        let owner = self.owner.0.lock().map_err(|_| MetadataError::WriteFailed)?;
        let store = owner.ready().map_err(|_| MetadataError::RemountRequired)?;
        metadata::reconcile(store).map_err(metadata_error)?;
        let start = scope(store);
        let mut catalog = Vec::new();
        let mut infos = Vec::new();
        let mut heads = Vec::new();
        for entry in store.entries().filter(|entry| entry.kind == ObjectKind::Ride && entry.flags == EntryFlags::NONE) {
            let info = store
                .with_source(entry.id, Some(entry.revision), |source| RideInfo::read(source))
                .map_err(|_| MetadataError::WriteFailed)?
                .map_err(|_| MetadataError::WriteFailed)?;
            heads.push((entry.id, entry.revision));
            infos.push((entry.id.0, info.clone()));
            let position = catalog.iter().position(|ride: &RideEntry| ride.id < entry.id.0).unwrap_or(catalog.len());
            if position < obc_app::UI_RIDES_CAP {
                if catalog.len() == obc_app::UI_RIDES_CAP {
                    catalog.pop();
                }
                catalog
                    .insert(position, RideEntry { id: entry.id.0, summary: RideSummary::from_info(&info, false, 0) });
            }
        }
        if !store.entries_ok() {
            return Err(MetadataError::WriteFailed);
        }
        let mut trips = RideTrips::new();
        for ride in &catalog {
            if let Some((_, info)) = infos.iter().find(|(id, _)| *id == ride.id) {
                RideTrip::note(&mut trips, info);
            }
        }
        metadata::read_rows(store, |row| {
            if row.kind == ObjectKind::Ride {
                if let Some(ride) = catalog.iter_mut().find(|ride| ride.id == row.id.0) {
                    ride.summary.synced = true;
                    ride.summary.synced_at_utc = row.timestamp;
                }
            }
        })
        .map_err(metadata_error)?;
        if scope(store) != start {
            return Err(MetadataError::Stale);
        }
        self.catalog = catalog;
        self.trips = trips;
        self.heads = heads;
        Ok(Some(start))
    }
    fn delete_by_id(&mut self, id: CatalogObjectId) -> Result<bool, CatalogError> {
        let Some(&(id, revision)) = self.heads.iter().find(|(candidate, _)| candidate.0 == id) else {
            return Ok(false);
        };
        match self.owner.remove(ObjectKind::Ride, id, revision) {
            Ok(()) => Ok(true),
            Err(obc_storage::flat::StoreError::NotFound) => Ok(false),
            Err(_) => Err(CatalogError::RemoveFailed),
        }
    }

    fn fill_track(
        &self,
        id: CatalogObjectId,
        profile: &mut Profile,
        facts: &mut obc_route::RideTrackFacts,
    ) -> Option<Vec<(i32, i32)>> {
        let &(id, revision) = self.heads.iter().find(|(candidate, _)| candidate.0 == id)?;
        let source = self.owner.open(id, revision).ok()?;
        let mut preview = Default::default();
        obc_route::ride_track_into::<{ obc_app::NAV_PREVIEW_MAX }>(&source, profile, facts, &mut preview).ok()?;
        Some(preview.to_vec())
    }
}

#[cfg(test)]
pub(crate) mod tests;
