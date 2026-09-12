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
}

impl FlatRouteStore {
    pub fn from_bytes(routes: &[&[u8]]) -> Result<Self, ImportError> {
        Self::new(HostStore::memory()?, routes)
    }

    /// Seed a session's routes. All subsequent route mutations go through this repository.
    pub fn new(owner: HostStore, routes: &[&[u8]]) -> Result<Self, ImportError> {
        let mut repo =
            Self { owner, catalog: Vec::new(), ids: Vec::new(), revisions: Vec::new(), active: None, nav_id: None };
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

    fn active_source(&self) -> Option<&dyn ByteSource> {
        self.active.as_ref().map(|s| s as &dyn ByteSource)
    }
    fn invalidate_active(&mut self) {
        self.active = None;
    }
}

#[cfg(test)]
mod tests;
