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
    unaccepted: u64,
    internal_routes: u64,
    /// The built trip day, as a catalog bit: at most one is set.
    built_day: u64,
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
            unaccepted: 0,
            internal_routes: 0,
            built_day: 0,
        };
        for meta in repo.owner.entries()? {
            if meta.kind == ObjectKind::Route {
                if repo.ids.len() == obc_app::MAX_ROUTES {
                    break;
                }
                let source = repo.owner.open(meta.id, meta.revision)?;
                let (summary, flags) = RouteSummary::read_with_flags(&source).map_err(|_| StoreError::Invalid)?;
                repo.publish(meta, summary, flags);
            }
        }
        for bytes in routes {
            repo.write(bytes, None)?;
        }
        Ok(repo)
    }

    pub fn source(&self, id: CatalogObjectId) -> Result<ObjectSource, StoreError> {
        let i = self.ids.iter().position(|&current| current == id).ok_or(StoreError::NotFound)?;
        self.owner.open(ObjectId(id), self.revisions[i])
    }

    /// Import a fresh route; input files remain an adapter concern.
    pub fn import(&mut self, bytes: &[u8]) -> Result<CatalogObjectId, ImportError> {
        self.write(bytes, None).map(|id| id.0)
    }

    /// Replace exactly the catalog revision currently held by this repository.
    pub fn replace(&mut self, id: CatalogObjectId, bytes: &[u8]) -> Result<(), ImportError> {
        let i = self.ids.iter().position(|&current| current == id).ok_or(StoreError::NotFound)?;
        self.write(bytes, Some((ObjectId(id), self.revisions[i]))).map(|_| ())
    }

    fn write(&mut self, bytes: &[u8], previous: Option<(ObjectId, Revision)>) -> Result<ObjectId, ImportError> {
        let (summary, flags) = RouteSummary::read_with_flags(&SliceSource(bytes)).map_err(|_| StoreError::Invalid)?;
        let meta = self.owner.import(
            ObjectKind::Route,
            previous,
            &mut &bytes[..],
            bytes.len() as u64,
            DisplayName::default(),
        )?;
        // No read or open can turn a committed write into a reported failure.
        self.publish(meta, summary, flags);
        Ok(meta.id)
    }

    fn publish(&mut self, meta: EntryMeta, summary: RouteSummary, flags: u8) {
        use obc_formats::obcr::{FLAG_ASSISTANT_CANDIDATE, FLAG_BUILT_DAY};
        let candidate = flags & FLAG_ASSISTANT_CANDIDATE != 0;
        let built = flags & FLAG_BUILT_DAY != 0;
        let i = if let Some(i) = self.ids.iter().position(|&id| id == meta.id.0) {
            self.revisions[i] = meta.revision;
            self.catalog[i] = summary;
            i
        } else {
            let i = self.ids.len();
            self.ids.push(meta.id.0);
            self.revisions.push(meta.revision);
            self.catalog.push(summary);
            i
        };
        if i < 64 {
            self.internal_routes &= !(1 << i);
            self.built_day &= !(1 << i);
            if candidate || built {
                self.internal_routes |= 1 << i;
            }
            if built {
                self.built_day |= 1 << i;
            }
            self.unaccepted &= !(1 << i);
            if candidate && !meta.flags.has(obc_storage::flat::EntryFlags::ASSISTANT_ACCEPTED) {
                self.unaccepted |= 1 << i;
            }
        }
    }
}

impl RouteRepository for FlatRouteStore {
    fn set_route_clock(&mut self, utc: Option<u32>) {
        if let Ok(owner) = self.owner.0.lock() {
            if let Ok(store) = owner.ready() {
                store.set_route_added_at(utc);
            }
        }
    }
    fn cleanup_route(
        &mut self,
        before_utc: u32,
        identity: obc_app::device_core::StoreIdentity,
        active: Option<CatalogObjectId>,
    ) -> Result<Option<CatalogObjectId>, CatalogError> {
        let mut owner = self.owner.0.lock().map_err(|_| CatalogError::Unreadable)?;
        let store = owner.ready().map_err(|_| CatalogError::RemountRequired)?;
        if scope(store).store != identity {
            return Err(CatalogError::Stale);
        }
        let Some((id, batch)) = obc_storage::flat::route_cleanup::next(store, before_utc, active.map(ObjectId))
            .map_err(|_| CatalogError::Unreadable)?
        else {
            return Ok(None);
        };
        owner.commit(&batch).map_err(|_| CatalogError::RemoveFailed)?;
        Ok(Some(id.0))
    }

    fn catalog(&self) -> &[RouteSummary] {
        &self.catalog
    }
    fn internal_routes(&self) -> u64 {
        self.internal_routes
    }
    fn unaccepted_routes(&self) -> u64 {
        self.unaccepted
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
    ) -> Result<Option<obc_app::device_core::StoreRevision>, obc_app::metadata::MetadataError> {
        use obc_storage::flat::{metadata, Store};
        let owner = self.owner.0.lock().map_err(|_| obc_app::metadata::MetadataError::WriteFailed)?;
        let store = owner.ready().map_err(|_| obc_app::metadata::MetadataError::RemountRequired)?;
        metadata::reconcile(store).map_err(metadata_error)?;
        let start = scope(store);
        let mut catalog = Vec::new();
        let mut ids = Vec::new();
        let mut revisions = Vec::new();
        let mut unaccepted = 0u64;
        let mut internal_routes = 0u64;
        let mut built_day = 0u64;
        for entry in store.entries().filter(|entry| entry.kind == ObjectKind::Route && entry.flags.is_route_head()) {
            if ids.len() == obc_app::MAX_ROUTES {
                break;
            }
            let (summary, flags) = store
                .with_source(entry.id, Some(entry.revision), |source| RouteSummary::read_with_flags(source))
                .map_err(|_| obc_app::metadata::MetadataError::WriteFailed)?
                .map_err(|_| obc_app::metadata::MetadataError::WriteFailed)?;
            let candidate = flags & obc_formats::obcr::FLAG_ASSISTANT_CANDIDATE != 0;
            if candidate || flags & obc_formats::obcr::FLAG_BUILT_DAY != 0 {
                internal_routes |= 1 << ids.len();
            }
            if flags & obc_formats::obcr::FLAG_BUILT_DAY != 0 {
                built_day |= 1 << ids.len();
            }
            if candidate && !entry.flags.has(obc_storage::flat::EntryFlags::ASSISTANT_ACCEPTED) {
                unaccepted |= 1 << ids.len();
            }
            catalog.push(summary);
            ids.push(entry.id.0);
            revisions.push(entry.revision);
        }
        if !store.entries_ok() || scope(store) != start {
            return Err(obc_app::metadata::MetadataError::Stale);
        }
        self.catalog = catalog;
        self.ids = ids;
        self.revisions = revisions;
        self.unaccepted = unaccepted;
        self.internal_routes = internal_routes;
        self.built_day = built_day;
        Ok(Some(start))
    }
    fn write_checkpoint(
        &mut self,
        scope: obc_app::device_core::StoreRevision,
        change: obc_app::navigator::CheckpointChange,
    ) -> Result<(), obc_app::metadata::MetadataError> {
        use obc_app::metadata::MetadataError;
        let owner = self.owner.0.lock().map_err(|_| MetadataError::WriteFailed)?;
        let store = owner.ready().map_err(|_| MetadataError::RemountRequired)?;
        obc_storage::flat::metadata::write_checkpoint(
            store,
            obc_storage::flat::StoreId(scope.store.bytes()),
            scope.revision.raw(),
            change.expected,
            change.next,
        )
        .map_err(metadata_error)
    }

    fn delete_by_id(&mut self, id: CatalogObjectId) -> Result<bool, CatalogError> {
        let entries = self.owner.entries().map_err(|error| catalog_error(&self.owner, error))?;
        let Some(meta) = entries.into_iter().find(|meta| meta.id.0 == id) else { return Ok(false) };
        if meta.kind != ObjectKind::Route {
            return Err(CatalogError::Unsupported);
        }
        let i = self.ids.iter().position(|&candidate| candidate == id).ok_or(CatalogError::Stale)?;
        let existed = match self.owner.remove(ObjectKind::Route, ObjectId(id), self.revisions[i]) {
            Ok(()) => true,
            Err(StoreError::NotFound) => false,
            Err(error) => return Err(catalog_error(&self.owner, error)),
        };
        self.revisions.remove(i);
        self.ids.remove(i);
        self.catalog.remove(i);
        if i < 64 {
            for mask in [&mut self.unaccepted, &mut self.internal_routes] {
                *mask = (*mask & ((1u64 << i) - 1)) | if i < 63 { (*mask >> (i + 1)) << i } else { 0 };
            }
        }
        if self.active.as_ref().is_some_and(|source| source.id().0 == id) {
            self.active = None;
        }
        if self.nav_id == Some(ObjectId(id)) {
            self.nav_id = None;
        }
        Ok(existed)
    }

    fn publish_nav_route(&mut self, bytes: &[u8]) -> Option<crate::RoutePublication> {
        let (summary, flags) = RouteSummary::read_with_flags(&SliceSource(bytes)).ok()?;
        // A built day replaces the one before it in place, so a card holds at most one.
        let previous = (flags & obc_formats::obcr::FLAG_BUILT_DAY != 0 && self.built_day != 0).then(|| {
            let i = self.built_day.trailing_zeros() as usize;
            (ObjectId(self.ids[i]), self.revisions[i])
        });
        let meta = self.owner.import_computed_route(bytes, previous).ok()?;
        self.publish(meta, summary, flags);
        Some(crate::RoutePublication {
            id: meta.id.0,
            revision: meta.revision.0,
            store: self.store_scope().map(|scope| scope.store),
        })
    }

    fn publish_review_route(
        &mut self,
        bytes: &[u8],
    ) -> Result<crate::RoutePublication, obc_app::navigator::NavigatorError> {
        use obc_app::navigator::NavigatorError;
        let (summary, flags) =
            RouteSummary::read_with_flags(&SliceSource(bytes)).map_err(|_| NavigatorError::Unavailable)?;
        let meta = self.owner.import_computed_route(bytes, None).map_err(|error| match error {
            crate::flat_store::ImportError::Storage(StoreError::Media | StoreError::ReadOnly)
            | crate::flat_store::ImportError::RemountRequired => NavigatorError::DurabilityUnknown,
            _ => NavigatorError::Store,
        })?;
        self.publish(meta, summary, flags);
        Ok(crate::RoutePublication {
            id: meta.id.0,
            revision: meta.revision.0,
            store: self.store_scope().map(|scope| scope.store),
        })
    }
    fn route_bytes(&self, id: CatalogObjectId) -> Option<Vec<u8>> {
        use obc_formats::io::ByteSource;
        let source = self.source(id).ok()?;
        let mut bytes = vec![0; usize::try_from(source.len()).ok()?];
        source.read_at(0, &mut bytes).ok()?;
        Some(bytes)
    }
    fn fingerprint(&self, id: CatalogObjectId) -> Option<obc_formats::assistant::PayloadFingerprint> {
        self.owner
            .entries()
            .ok()?
            .into_iter()
            .find(|meta| meta.kind == ObjectKind::Route && meta.id.0 == id)
            .map(obc_storage::flat::metadata::fingerprint)
    }
    fn resume_map_matches(&self, map: Option<obc_formats::obcr::RouteSourceKey>) -> bool {
        let Ok(Some(checkpoint)) = self.read_checkpoint() else {
            return false;
        };
        let Ok(source) = self.owner.open(ObjectId(checkpoint.route.object), Revision(checkpoint.route.revision)) else {
            return false;
        };
        obc_route::RouteObjectInfo::read(&source)
            .is_ok_and(|info| info.attribution_map.is_none_or(|attribution| Some(attribution) == map))
    }
    fn read_checkpoint(
        &self,
    ) -> Result<Option<obc_formats::assistant::NavigatorCheckpoint>, obc_app::metadata::MetadataError> {
        use obc_app::metadata::MetadataError;
        let owner = self.owner.0.lock().map_err(|_| MetadataError::WriteFailed)?;
        let store = owner.ready().map_err(|_| MetadataError::RemountRequired)?;
        obc_storage::flat::metadata::read_checkpoint(store).map_err(metadata_error)
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

    fn retract_reviews(&mut self, ids: &[CatalogObjectId]) -> Result<(), CatalogError> {
        let batch: Vec<_> = ids
            .iter()
            .filter_map(|id| self.ids.iter().position(|candidate| candidate == id))
            .map(|index| (ObjectId(self.ids[index]), self.revisions[index]))
            .take(obc_storage::flat::store::MAX_BATCH)
            .collect();
        match self.owner.remove_routes(&batch) {
            Ok(()) => {
                self.refresh_metadata().map_err(|_| CatalogError::Unreadable)?;
                Ok(())
            }
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
    fn pin_review(&self, source: obc_formats::obcr::RouteSourceKey) -> Option<crate::RouteLease> {
        if self.store_scope()?.store.bytes() != source.store {
            return None;
        }
        let source = self.owner.open(ObjectId(source.object), Revision(source.revision)).ok()?;
        source.is_current().then_some(crate::RouteLease::Flat(source))
    }
    fn invalidate_active(&mut self) {
        self.active = None;
    }
}

#[cfg(test)]
mod tests;

pub(crate) fn scope<D: obc_storage::flat::BlockDevice>(
    store: &obc_storage::flat::FlatStore<D>,
) -> obc_app::device_core::StoreRevision {
    obc_app::device_core::StoreRevision {
        store: obc_app::device_core::StoreIdentity::from_bytes(store.store_id().0),
        revision: obc_app::device_core::Revision::new(store.sequence()),
    }
}

pub(crate) fn metadata_error(error: obc_storage::flat::metadata::Error) -> obc_app::metadata::MetadataError {
    use obc_app::metadata::MetadataError as E;
    use obc_storage::flat::metadata::Error;
    match error {
        Error::WrongStore | Error::Stale => E::Stale,
        Error::RemountRequired => E::RemountRequired,
        Error::Store(StoreError::Busy) => E::Busy,
        _ => E::WriteFailed,
    }
}

pub(crate) fn catalog_error(owner: &HostStore, error: StoreError) -> CatalogError {
    if matches!(owner.mode(), Ok(obc_storage::flat::Mode::RemountRequired) | Err(_)) {
        return CatalogError::RemountRequired;
    }
    match error {
        StoreError::ReadOnly => CatalogError::Unsupported,
        StoreError::RevisionConflict { .. } => CatalogError::Stale,
        _ => CatalogError::Unreadable,
    }
}
