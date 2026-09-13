//! Read-side ride catalog and retention executor on one shared card. Recording is not supported.

use crate::{
    flat_routes::{metadata_error, scope},
    flat_store::HostStore,
    RideRepository, TrackRepository,
};
use obc_app::{
    catalog_state::CatalogError,
    device_core::StoreRevision,
    recorder::RideClose,
    retention::{RetentionEffect, RetentionError},
    CatalogObjectId, RideEntry, RideRetentionRecord, RideSummary,
};
use obc_route::{Profile, RideInfo, RideStats};
use obc_storage::flat::{metadata, EntryFlags, ObjectId, ObjectKind, Revision, Store, StoreId};

pub struct FlatRideStore {
    owner: HostStore,
    catalog: Vec<RideEntry>,
    inventory: Vec<RideRetentionRecord>,
    heads: Vec<(ObjectId, Revision)>,
}

impl FlatRideStore {
    pub fn new(owner: HostStore) -> Result<Self, RetentionError> {
        let mut repo = Self { owner, catalog: Vec::new(), inventory: Vec::new(), heads: Vec::new() };
        repo.refresh_metadata()?;
        Ok(repo)
    }
}

impl RideRepository for FlatRideStore {
    fn catalog(&self) -> &[RideEntry] {
        &self.catalog
    }

    fn retention_inventory(&self) -> Option<&[RideRetentionRecord]> {
        Some(&self.inventory)
    }

    fn store_scope(&self) -> Option<StoreRevision> {
        let owner = self.owner.0.lock().ok()?;
        Some(scope(owner.ready().ok()?))
    }

    fn refresh_metadata(&mut self) -> Result<Option<StoreRevision>, RetentionError> {
        let owner = self.owner.0.lock().map_err(|_| RetentionError::WriteFailed)?;
        let store = owner.ready().map_err(|_| RetentionError::RemountRequired)?;
        metadata::reconcile(store).map_err(metadata_error)?;
        let start = scope(store);
        let mut catalog = Vec::new();
        let mut inventory = Vec::new();
        let mut heads = Vec::new();
        for entry in store.entries().filter(|entry| entry.kind == ObjectKind::Ride && entry.flags == EntryFlags::NONE) {
            if inventory.len() == obc_app::MAX_RIDES {
                return Err(RetentionError::WriteFailed);
            }
            let info = store
                .with_source(entry.id, Some(entry.revision), |source| RideInfo::read(source))
                .map_err(|_| RetentionError::WriteFailed)?
                .map_err(|_| RetentionError::WriteFailed)?;
            inventory.push(RideRetentionRecord { id: entry.id.0, synced: false, synced_at_utc: 0 });
            heads.push((entry.id, entry.revision));
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
            return Err(RetentionError::WriteFailed);
        }
        metadata::read_rows(store, |row| {
            if row.kind == ObjectKind::Ride {
                if let Some(record) = inventory.iter_mut().find(|record| record.id == row.id.0) {
                    record.synced = true;
                    record.synced_at_utc = row.timestamp;
                }
                if let Some(ride) = catalog.iter_mut().find(|ride| ride.id == row.id.0) {
                    ride.summary.synced = true;
                    ride.summary.synced_at_utc = row.timestamp;
                }
            }
        })
        .map_err(metadata_error)?;
        if scope(store) != start {
            return Err(RetentionError::Stale);
        }
        self.catalog = catalog;
        self.inventory = inventory;
        self.heads = heads;
        Ok(Some(start))
    }

    fn write_metadata(&mut self, effect: RetentionEffect) -> Result<(), RetentionError> {
        let RetentionEffect::WriteRideMetadata { scope: Some(expected), id, synced_at, .. } = effect else {
            return Err(RetentionError::Unsupported);
        };
        let owner = self.owner.0.lock().map_err(|_| RetentionError::WriteFailed)?;
        let store = owner.ready().map_err(|_| RetentionError::RemountRequired)?;
        metadata::write_ride(store, StoreId(expected.store.bytes()), expected.revision.raw(), ObjectId(id), synced_at)
            .map_err(metadata_error)
    }

    fn expire_ride(&mut self, id: CatalogObjectId, expected: StoreRevision) -> Result<bool, CatalogError> {
        let owner = self.owner.0.lock().map_err(|_| CatalogError::RemoveFailed)?;
        let store = owner.ready().map_err(|_| CatalogError::RemountRequired)?;
        metadata::remove_ride(store, StoreId(expected.store.bytes()), expected.revision.raw(), ObjectId(id))
            .map(|()| true)
            .map_err(|error| match metadata_error(error) {
                RetentionError::Stale => CatalogError::Stale,
                RetentionError::RemountRequired => CatalogError::RemountRequired,
                _ => CatalogError::RemoveFailed,
            })
    }

    fn delete_by_id(&mut self, id: CatalogObjectId) -> Result<bool, CatalogError> {
        let Some(&(id, revision)) = self.heads.iter().find(|(candidate, _)| candidate.0 == id) else {
            return Ok(false);
        };
        self.owner.remove(ObjectKind::Ride, id, revision).map_err(|_| CatalogError::RemoveFailed)?;
        Ok(true)
    }

    fn fill_track(&self, id: CatalogObjectId, profile: &mut Profile) -> Option<Vec<(i32, i32)>> {
        let &(id, revision) = self.heads.iter().find(|(candidate, _)| candidate.0 == id)?;
        let source = self.owner.open(id, revision).ok()?;
        let mut preview = Default::default();
        obc_route::ride_track_into::<{ obc_app::NAV_PREVIEW_MAX }>(&source, profile, &mut preview).ok()?;
        Some(preview.to_vec())
    }
}

impl TrackRepository for FlatRideStore {
    fn open(&mut self, _session: u32, _name: Option<&str>) -> bool {
        false
    }
    fn finalize(&mut self, _stats: RideStats) -> RideClose {
        RideClose::Failed
    }
    fn discard(&mut self) -> bool {
        false
    }
    fn checkpoint(&mut self) -> bool {
        false
    }
    fn append(&mut self, _point: obc_ports::TrackPoint) -> bool {
        false
    }
}

#[cfg(test)]
pub(crate) mod tests;
