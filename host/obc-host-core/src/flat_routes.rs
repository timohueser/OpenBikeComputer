//! Route catalog projections and active readers from one shared session card.

use crate::{
    flat_store::{HostStore, ImportError, ObjectSource},
    RouteRepository,
};
use obc_app::{catalog_state::CatalogError, CatalogObjectId};
use obc_formats::io::{ByteSource, SliceSource};
use obc_route::RouteSummary;
use obc_storage::flat::{DisplayName, EntryMeta, ObjectId, ObjectKind, Revision, StoreError};

pub struct FlatRouteStore {
    owner: HostStore,
    catalog: Vec<RouteSummary>,
    ids: Vec<CatalogObjectId>,
    revisions: Vec<Revision>,
    active: Option<ObjectSource>,
    nav_id: Option<ObjectId>,
    metadata: Vec<obc_app::RouteRetentionMeta>,
}

impl FlatRouteStore {
    pub fn from_bytes(routes: &[&[u8]]) -> Result<Self, ImportError> {
        Self::new(HostStore::memory()?, routes)
    }

    /// Seed a session's routes. All subsequent route mutations go through this repository.
    pub fn new(owner: HostStore, routes: &[&[u8]]) -> Result<Self, ImportError> {
        let mut repo = Self {
            owner,
            catalog: Vec::new(),
            ids: Vec::new(),
            revisions: Vec::new(),
            active: None,
            nav_id: None,
            metadata: Vec::new(),
        };
        for meta in repo.owner.entries()? {
            if meta.kind == ObjectKind::Route {
                let source = repo.owner.open(meta.id, meta.revision)?;
                let summary = RouteSummary::read(&source).map_err(|_| StoreError::Invalid)?;
                repo.publish(meta, summary);
            }
        }
        for bytes in routes {
            repo.write(bytes, None)?;
        }
        Ok(repo)
    }

    fn write(&mut self, bytes: &[u8], previous: Option<(ObjectId, Revision)>) -> Result<ObjectId, ImportError> {
        let summary = RouteSummary::read(&SliceSource(bytes)).map_err(|_| StoreError::Invalid)?;
        let meta = self.owner.import(
            ObjectKind::Route,
            previous,
            &mut &bytes[..],
            bytes.len() as u64,
            DisplayName::default(),
        )?;
        // No read or open can turn a committed write into a reported failure.
        self.publish(meta, summary);
        Ok(meta.id)
    }

    fn publish(&mut self, meta: EntryMeta, summary: RouteSummary) {
        if let Some(i) = self.ids.iter().position(|&id| id == meta.id.0) {
            self.revisions[i] = meta.revision;
            self.catalog[i] = summary;
        } else {
            self.ids.push(meta.id.0);
            self.revisions.push(meta.revision);
            self.catalog.push(summary);
        }
    }
}

impl RouteRepository for FlatRouteStore {
    fn catalog(&self) -> &[RouteSummary] {
        &self.catalog
    }
    fn ids(&self) -> &[CatalogObjectId] {
        &self.ids
    }

    fn store_scope(&self) -> Option<obc_app::device_core::StoreRevision> {
        let owner = self.owner.0.lock().ok()?;
        let store = owner.ready().ok()?;
        Some(scope(store))
    }

    fn refresh_metadata(
        &mut self,
    ) -> Result<Option<obc_app::device_core::StoreRevision>, obc_app::retention::RetentionError> {
        use obc_storage::flat::{metadata, Store};
        let owner = self.owner.0.lock().map_err(|_| obc_app::retention::RetentionError::WriteFailed)?;
        let store = owner.ready().map_err(|_| obc_app::retention::RetentionError::RemountRequired)?;
        metadata::reconcile(store).map_err(metadata_error)?;
        let start = scope(store);
        let mut rows = Vec::new();
        metadata::read_routes(store, |row| rows.push(row)).map_err(metadata_error)?;
        let mut catalog = Vec::new();
        let mut ids = Vec::new();
        let mut revisions = Vec::new();
        let mut metas = Vec::new();
        for entry in store
            .entries()
            .filter(|entry| entry.kind == ObjectKind::Route && entry.flags == obc_storage::flat::EntryFlags::NONE)
        {
            let summary = store
                .with_source(entry.id, Some(entry.revision), |source| RouteSummary::read(source))
                .map_err(|_| obc_app::retention::RetentionError::WriteFailed)?
                .map_err(|_| obc_app::retention::RetentionError::WriteFailed)?;
            catalog.push(summary);
            ids.push(entry.id.0);
            revisions.push(entry.revision);
            metas.push(
                rows.iter()
                    .find(|row| row.id == entry.id)
                    .map(|row| {
                        obc_app::RouteRetentionMeta::new(obc_app::Retention::from_u8(row.retention), row.timestamp)
                    })
                    .unwrap_or_default(),
            );
        }
        if !store.entries_ok() || scope(store) != start {
            return Err(obc_app::retention::RetentionError::Stale);
        }
        self.catalog = catalog;
        self.ids = ids;
        self.revisions = revisions;
        self.metadata = metas;
        Ok(Some(start))
    }

    fn retention_metas(&self) -> Vec<obc_app::RouteRetentionMeta> {
        self.metadata.clone()
    }

    fn write_metadata(
        &mut self,
        effect: obc_app::retention::RetentionEffect,
    ) -> Result<(), obc_app::retention::RetentionError> {
        use obc_app::retention::{RetentionEffect, RetentionError};
        let RetentionEffect::WriteRouteMetadata { scope: Some(expected), id, meta, .. } = effect else {
            return Err(RetentionError::Unsupported);
        };
        let owner = self.owner.0.lock().map_err(|_| RetentionError::WriteFailed)?;
        let store = owner.ready().map_err(|_| RetentionError::RemountRequired)?;
        obc_storage::flat::metadata::write_route(
            store,
            obc_storage::flat::StoreId(expected.store.bytes()),
            expected.revision.raw(),
            ObjectId(id),
            meta.retention as u8,
            meta.last_used_utc,
        )
        .map_err(metadata_error)
    }

    fn expire_route(
        &mut self,
        id: CatalogObjectId,
        expected: obc_app::device_core::StoreRevision,
    ) -> Result<bool, CatalogError> {
        let owner = self.owner.0.lock().map_err(|_| CatalogError::RemoveFailed)?;
        let store = owner.ready().map_err(|_| CatalogError::RemountRequired)?;
        obc_storage::flat::metadata::remove_route(
            store,
            obc_storage::flat::StoreId(expected.store.bytes()),
            expected.revision.raw(),
            ObjectId(id),
        )
        .map(|()| true)
        .map_err(|error| match metadata_error(error) {
            obc_app::retention::RetentionError::Stale => CatalogError::Stale,
            obc_app::retention::RetentionError::RemountRequired => CatalogError::RemountRequired,
            _ => CatalogError::RemoveFailed,
        })
    }

    fn delete_by_id(&mut self, id: CatalogObjectId) -> Result<bool, CatalogError> {
        // Only this complete route projection grants ownership; a map or ride ID is not ours.
        let Some(i) = self.ids.iter().position(|&candidate| candidate == id) else { return Ok(false) };
        let existed = match self.owner.remove(ObjectKind::Route, ObjectId(id), self.revisions[i]) {
            Ok(()) => true,
            Err(StoreError::NotFound) => false,
            Err(_) => return Err(CatalogError::RemoveFailed),
        };
        self.revisions.remove(i);
        self.ids.remove(i);
        self.catalog.remove(i);
        if self.active.as_ref().is_some_and(|source| source.id().0 == id) {
            self.active = None;
        }
        if self.nav_id == Some(ObjectId(id)) {
            self.nav_id = None;
        }
        Ok(existed)
    }

    fn publish_nav_route(&mut self, bytes: &[u8]) -> Option<crate::RoutePublication> {
        let id = self.write_nav_route(bytes)?;
        let i = self.ids.iter().position(|&candidate| candidate == id)?;
        Some(crate::RoutePublication {
            id,
            revision: self.revisions[i].0,
            store: self.store_scope().map(|scope| scope.store),
        })
    }

    fn retract_nav_route(&mut self, publication: crate::RoutePublication) -> Result<(), CatalogError> {
        if self.store_scope().map(|scope| scope.store) != publication.store {
            return Err(CatalogError::Stale);
        }
        match self.owner.remove(ObjectKind::Route, ObjectId(publication.id), Revision(publication.revision)) {
            Ok(()) => {
                self.refresh_metadata().map_err(|_| CatalogError::Unreadable)?;
                Ok(())
            }
            Err(StoreError::NotFound | StoreError::RevisionConflict { .. }) => Ok(()),
            Err(StoreError::ReadOnly) => Err(CatalogError::RemountRequired),
            Err(_) => Err(CatalogError::RemoveFailed),
        }
    }

    fn write_nav_route(&mut self, bytes: &[u8]) -> Option<CatalogObjectId> {
        let previous = self
            .nav_id
            .and_then(|id| self.ids.iter().position(|&candidate| candidate == id.0).map(|i| (id, self.revisions[i])));
        let id = self.write(bytes, previous).ok()?;
        self.nav_id = Some(id);
        Some(id.0)
    }

    fn sync_active(&mut self, want: Option<usize>) -> bool {
        let desired = want
            .and_then(|i| self.ids.get(i).zip(self.revisions.get(i)))
            .map(|(&id, &revision)| (ObjectId(id), revision));
        if self.active.as_ref().is_some_and(|source| {
            desired.is_some_and(|(id, revision)| source.id() == id && source.revision() == revision)
        }) {
            return false;
        }
        // Opening is separate from commit. Failure clears a mismatched old source and leaves the
        // desired revision pending; the next call can retry without reporting a false write failure.
        let next = desired.and_then(|(id, revision)| self.owner.open(id, revision).ok());
        let changed = self.active.is_some() || next.is_some();
        self.active = next;
        changed
    }

    fn pin_active(&self) -> Option<crate::RouteLease> {
        self.active.clone().map(crate::RouteLease::Flat)
    }

    fn active_source(&self) -> Option<&dyn ByteSource> {
        self.active.as_ref().map(|s| s as &dyn ByteSource)
    }
    fn invalidate_active(&mut self) {
        self.active = None;
    }
}

#[cfg(test)]
mod tests;

fn scope<D: obc_storage::flat::BlockDevice>(
    store: &obc_storage::flat::FlatStore<D>,
) -> obc_app::device_core::StoreRevision {
    obc_app::device_core::StoreRevision {
        store: obc_app::device_core::StoreIdentity::from_bytes(store.store_id().0),
        revision: obc_app::device_core::Revision::new(store.sequence()),
    }
}

fn metadata_error(error: obc_storage::flat::metadata::Error) -> obc_app::retention::RetentionError {
    use obc_app::retention::RetentionError as E;
    use obc_storage::flat::metadata::Error;
    match error {
        Error::WrongStore | Error::Stale => E::Stale,
        Error::RemountRequired => E::RemountRequired,
        Error::Store(StoreError::Busy) => E::Busy,
        _ => E::WriteFailed,
    }
}
