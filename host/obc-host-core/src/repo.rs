//! The narrow repository interfaces the shared typed executor ([`crate::HostLoop`]) drives — one
//! trait per store family the app talks to, so the sequencing (delete, rescan, re-feed; the nav
//! commit order; the track lifecycle) lives once in [`crate::dispatch`] and every host store plugs
//! in behind the same shape. Storage internals stay in the concrete stores; these traits carry no
//! `std`-vs-`no_std` assumptions of their own.
//!
//! The board deliberately does not implement these because it owns its own async loop. Protocol
//! tests pin the command and event semantics that both implementations share.

use obc_app::catalog_state::CatalogError;
use obc_app::recorder::{CheckpointStatus, RecorderError, RideClose, RideContinuation};
use obc_app::{App, CatalogObjectId, RideEntry};
use obc_formats::io::ByteSource;
use obc_ports::TrackPoint;
use obc_route::{Profile, RideStats, RideTrackFacts, RouteSummary};

/// Exact publication returned before the executor reports a committed route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutePublication {
    pub id: CatalogObjectId,
    pub revision: u64,
    pub store: Option<obc_app::device_core::StoreIdentity>,
}

/// An immutable active-route snapshot retained through detour preview and commit.
#[derive(Clone)]
pub enum RouteLease {
    Flat(crate::flat_store::ObjectSource),
    Memory { id: CatalogObjectId, bytes: std::sync::Arc<[u8]> },
}
impl RouteLease {
    pub fn id(&self) -> CatalogObjectId {
        match self {
            Self::Flat(source) => source.id().0,
            Self::Memory { id, .. } => *id,
        }
    }
    pub fn matches(&self, current: &Self) -> bool {
        match (self, current) {
            (Self::Flat(a), Self::Flat(b)) => a.same_revision(b) && a.is_current(),
            (Self::Memory { id: a, bytes: ab }, Self::Memory { id: b, bytes: bb }) => {
                a == b && std::sync::Arc::ptr_eq(ab, bb)
            }
            _ => false,
        }
    }
}
impl ByteSource for RouteLease {
    fn len(&self) -> u64 {
        match self {
            Self::Flat(source) => source.len(),
            Self::Memory { bytes, .. } => bytes.len() as u64,
        }
    }
    fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), obc_formats::io::Error> {
        match self {
            Self::Flat(source) => source.read_at(offset, out),
            Self::Memory { bytes, .. } => obc_formats::io::SliceSource(bytes).read_at(offset, out),
        }
    }
}

/// Route projections, retained readers and physical writes used by the shared host executor.
pub trait RouteRepository {
    fn set_route_clock(&mut self, _utc: Option<u32>) {}
    fn cleanup_route(
        &mut self,
        _before_utc: u32,
        _store: obc_app::device_core::StoreIdentity,
        _active: Option<CatalogObjectId>,
    ) -> Result<Option<CatalogObjectId>, CatalogError> {
        Err(CatalogError::Unsupported)
    }

    /// The route catalog (summaries), for [`App::set_routes_with_ids`](obc_app::App::set_routes_with_ids).
    fn catalog(&self) -> &[RouteSummary];
    /// Each catalog entry's session-stable durable id, parallel to [`catalog`](RouteRepository::catalog).
    fn ids(&self) -> &[CatalogObjectId];
    fn internal_routes(&self) -> u64 {
        0
    }
    fn temporary_routes(&self) -> u64 {
        0
    }
    fn unaccepted_routes(&self) -> u64 {
        0
    }
    fn write_checkpoint(
        &mut self,
        _scope: obc_app::device_core::StoreRevision,
        _change: obc_app::navigator::CheckpointChange,
    ) -> Result<(), obc_app::metadata::MetadataError> {
        Err(obc_app::metadata::MetadataError::Unsupported)
    }
    /// Remove the route: `Ok(true)` = removed, `Ok(false)` = already absent. A storage failure
    /// returns `Err`, so the executor cannot publish successful absence or probe another family.
    fn delete_by_id(&mut self, id: CatalogObjectId) -> Result<bool, CatalogError>;
    /// Persist the router's emitted OBCR as the reserved nav route (overwriting any previous plan),
    /// returning its session-stable id — or `None` on an I/O failure.
    fn write_nav_route(&mut self, bytes: &[u8]) -> Option<CatalogObjectId>;
    /// Publish with a compensation key. A store without exact observed revisions refuses it.
    fn publish_nav_route(&mut self, _bytes: &[u8]) -> Option<RoutePublication> {
        None
    }
    fn publish_review_route(&mut self, _bytes: &[u8]) -> Result<RoutePublication, obc_app::navigator::NavigatorError> {
        Err(obc_app::navigator::NavigatorError::Unavailable)
    }
    fn fingerprint(&self, _id: CatalogObjectId) -> Option<obc_formats::assistant::PayloadFingerprint> {
        None
    }
    /// The current bytes of stored route `id`.
    fn route_bytes(&self, _id: CatalogObjectId) -> Option<Vec<u8>> {
        None
    }
    /// Whether the stored checkpoint's route still accepts `map` as its attribution — the current
    /// map key, or `None` when no current map is open. An imported route attributes no map.
    fn resume_map_matches(&self, _map: Option<obc_formats::obcr::RouteSourceKey>) -> bool {
        false
    }
    fn read_checkpoint(
        &self,
    ) -> Result<Option<obc_formats::assistant::NavigatorCheckpoint>, obc_app::metadata::MetadataError> {
        Err(obc_app::metadata::MetadataError::Unsupported)
    }
    /// Remove only this publication. A replacement is already outside this operation's authority.
    fn retract_nav_route(&mut self, _publication: RoutePublication) -> Result<(), CatalogError> {
        Err(CatalogError::Unsupported)
    }
    /// Remove unused generated routes, at most one commit per call.
    fn retract_generated_routes(&mut self, _ids: &[CatalogObjectId]) -> Result<(), CatalogError> {
        Err(CatalogError::Unsupported)
    }
    /// Make the active route match `want`, (re)reading its bytes only on a change. **Returns whether
    /// the active bytes were (re)loaded this call** — the signal [`ActiveRouteSession`](crate::ActiveRouteSession)
    /// gates its index reparse on, so a settled view never reparses.
    fn sync_active(&mut self, want: Option<usize>) -> bool;
    /// A [`ByteSource`](obc_formats::io::ByteSource) over the active route's bytes.
    fn active_source(&self) -> Option<&dyn ByteSource>;
    /// Retain the exact active snapshot. Repositories without leases cannot plan detours.
    fn pin_active(&self) -> Option<RouteLease> {
        None
    }
    /// Force the active bytes to re-read on the next [`sync_active`](RouteRepository::sync_active)
    /// even under an unchanged index — a re-route rewrites the nav bytes beneath the same catalog slot.
    fn invalidate_active(&mut self);
    /// Current physical card identity/sequence, if this repository owns a flat card.
    fn store_scope(&self) -> Option<obc_app::device_core::StoreRevision> {
        None
    }
    /// Reload catalog and metadata together; only a complete validated projection returns a scope.
    fn refresh_metadata(
        &mut self,
    ) -> Result<Option<obc_app::device_core::StoreRevision>, obc_app::metadata::MetadataError> {
        Ok(None)
    }
}

/// The ride catalog (the Rides screen) plus the per-ride track reads its detail draws.
pub trait RideRepository {
    /// The ride catalog (paired entries, newest first), for [`App::set_rides`](obc_app::App::set_rides).
    fn catalog(&self) -> &[RideEntry];
    /// The names of the catalog's trips, one per trip key.
    fn trip_names(&self) -> &[obc_app::RideTrip] {
        &[]
    }
    /// Remove the ride: `Ok(true)` = removed, `Ok(false)` = already absent, `Err` = storage failure.
    fn delete_by_id(&mut self, id: CatalogObjectId) -> Result<bool, CatalogError>;
    /// Fill the keyed ride's profile and facts in place and return its preview from one track
    /// read. `None` = unknown/unreadable; the caller must not publish the profile on failure.
    fn fill_track(
        &self,
        id: CatalogObjectId,
        profile: &mut Profile,
        facts: &mut RideTrackFacts,
    ) -> Option<Vec<(i32, i32)>>;
    /// Re-scan after a ride was just saved. Folder-backed simulator stores use this hook; a static
    /// in-memory catalog is a no-op.
    fn refresh(&mut self) {}
    /// Complete catalog and durable policy refresh. Legacy stores have no card authority.
    fn refresh_metadata(
        &mut self,
    ) -> Result<Option<obc_app::device_core::StoreRevision>, obc_app::metadata::MetadataError> {
        self.refresh();
        Ok(None)
    }
    fn store_scope(&self) -> Option<obc_app::device_core::StoreRevision> {
        None
    }
}

/// The physical batch result; no accepted prefix is hidden by a later refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendStatus {
    Cancelled,
    Accepted(u16),
    NeedsCheckpoint,
}

/// The open ride object the app records into while riding — one method per
/// [`RecorderEffect`](obc_app::recorder::RecorderEffect), plus the session edge that opens the
/// object.
///
/// There is no `reconcile`: the recorder lifecycle is Recorder's, and a store that reconstructed it
/// from an action plus a session id would be deciding it a second time. There is no sink either:
/// the app stages its own samples and this writes the ones it is handed.
pub trait TrackRepository {
    /// Open a ride object for `session`, to be saved under `name`. Return true only when that
    /// object is attached. False leaves the open owed; an uncertain publication may still exist
    /// and its owner must refuse further writes until remount. Recovery attaches the same object.
    fn open(&mut self, session: u32, name: Option<&str>, now_ms: u32) -> bool;

    /// Close the open ride into a durable ride object.
    ///
    /// [`RideClose::Failed`] means the ride is **still there** and Recorder re-offers the same
    /// close, so a store must not throw the bytes away on the way out; [`RideClose::Nothing`] is
    /// how a store says there was no object to close, which is over rather than owed.
    fn finalize(&mut self, stats: RideStats) -> RideClose;

    /// Delete the open ride and its journal. Return success only after confirmed removal.
    /// A write error permits retry; ReadOnly tells Recorder that this boot cannot mutate the object.
    fn discard(&mut self) -> Result<(), RecorderError>;

    /// Checkpoint the accepted payload boundary. `continuation` is fresh only with no App-staged
    /// samples; otherwise retain the context accepted with the payload. A failed attempt must
    /// replay its frozen bytes and context before considering either argument again.
    fn checkpoint(
        &mut self,
        _stats: RideStats,
        _continuation: Option<RideContinuation>,
    ) -> Result<CheckpointStatus, RecorderError> {
        Ok(CheckpointStatus::Unsupported)
    }

    /// Accept a batch and its observation boundary. Legacy adapters report their actual prefix.
    fn append_batch(
        &mut self,
        points: &[TrackPoint],
        _continuation: Option<RideContinuation>,
    ) -> Result<AppendStatus, RecorderError> {
        let written = points.iter().take_while(|point| self.append(**point)).count() as u16;
        if written == 0 && !points.is_empty() {
            Err(RecorderError::Write)
        } else {
            Ok(AppendStatus::Accepted(written))
        }
    }

    /// Append one staged sample to the open ride. `false` means the medium refused it: Recorder
    /// keeps that sample and every sample behind it staged, and offers them again.
    ///
    /// A store with no log has nothing to write and says so by succeeding.
    fn append(&mut self, point: TrackPoint) -> bool {
        let _ = point;
        true
    }
}

/// Trip projections and physical removal. A host without trips uses the unit implementation.
pub trait TripCatalog {
    fn store_scope(&self) -> Option<obc_app::device_core::StoreRevision> {
        None
    }

    /// Delete only the trip object with id `id`. The cascade over member
    /// Delete only the trip object with id `id`. The cascade over member routes is
    /// `CatalogMachine`'s ordering and reaches this executor as its own removals, so there is no
    /// member lookup here. `Ok(true)` = removed, `Ok(false)` = absent, `Err` = failure.
    fn delete_by_id(&mut self, id: CatalogObjectId) -> Result<bool, CatalogError> {
        let _ = id;
        Ok(false)
    }
    /// Load a complete trip projection, preserving the previous one on failure.
    fn rescan(&mut self) -> Result<(), CatalogError> {
        Ok(())
    }
    /// Re-feed the app's trip list ([`App::set_trips`](obc_app::App::set_trips)) and progress
    /// records — call **after** the route catalog is re-fed so the stage ids resolve.
    fn refeed(&self, app: &mut App) {
        let _ = app;
    }
    /// Day `k` of the stored trip with key `key`.
    fn day(&self, key: u64, k: u16) -> Option<obc_route::TripDay> {
        let _ = (key, k);
        None
    }
    /// Write one trip progress record by the bound rules; `keys` are the stored trips' keys.
    fn write_progress(
        &mut self,
        record: obc_app::trip::TripProgress,
        keys: &[u64],
    ) -> Result<(), obc_app::metadata::MetadataError> {
        let _ = (record, keys);
        Err(obc_app::metadata::MetadataError::Unsupported)
    }
}

/// The trip-less host: the web demo and any test that drives routes/rides without `.obt` grouping.
impl TripCatalog for () {}
