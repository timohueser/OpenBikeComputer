//! The narrow repository interfaces the shared typed executor ([`crate::HostLoop`]) drives — one
//! trait per store family the app talks to, so the *sequencing* (delete → rescan → re-feed, the
//! nav commit order, the track lifecycle) lives once in [`crate::dispatch`] and every host store —
//! the simulator's folder-backed stores and the in-memory [`FlatRouteStore`](crate::FlatRouteStore)
//! family — plugs in behind the same shape. Storage internals stay in the concrete stores; these
//! traits carry no `std`-vs-`no_std` assumptions of their own.
//!
//! The board deliberately does **not** implement these (its FAT/`ObjectStore` path stays async and
//! board-specific — #801 non-goal, #809 owns the board loop); the *command/event semantics* it
//! shares are pinned by protocol tests instead.

use obc_app::catalog_state::CatalogError;
use obc_app::recorder::RideClose;
use obc_app::{App, CatalogObjectId, RideEntry, RouteRetentionMeta};
use obc_formats::io::ByteSource;
use obc_ports::TrackPoint;
use obc_route::{Profile, RideStats, RouteSummary};

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

/// The route catalog + the one active route's bytes, plus the reserved nav-route commit slot the
/// router writes into. Supersedes the old `NavRouteStore` (which was only the nav-commit slice):
/// the dispatcher needs the whole delete/rescan/active surface, so it lives in one trait.
pub trait RouteRepository {
    /// The route catalog (summaries), for [`App::set_routes_with_ids`](obc_app::App::set_routes_with_ids).
    fn catalog(&self) -> &[RouteSummary];
    /// Each catalog entry's session-stable durable id, parallel to [`catalog`](RouteRepository::catalog).
    fn ids(&self) -> &[CatalogObjectId];
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
    /// Remove only this publication. A replacement is already outside this operation's authority.
    fn retract_nav_route(&mut self, _publication: RoutePublication) -> Result<(), CatalogError> {
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
    /// Each catalog entry's device-local retention meta (epic #638, S3), parallel to
    /// [`ids`](RouteRepository::ids) — fed alongside the catalog through
    /// [`App::set_routes_with_meta`](obc_app::App::set_routes_with_meta) so the auto-expiry sweep
    /// reads device truth. Defaults to empty → every route reads
    /// [`Never`](obc_app::Retention::Never) (nothing expires), which a retention-less host keeps.
    fn retention_metas(&self) -> Vec<RouteRetentionMeta> {
        Vec::new()
    }
    /// Current physical card identity/sequence, if this repository owns a flat card.
    fn store_scope(&self) -> Option<obc_app::device_core::StoreRevision> {
        None
    }
    /// Reload catalog and metadata together; only a complete validated projection returns a scope.
    fn refresh_metadata(
        &mut self,
    ) -> Result<Option<obc_app::device_core::StoreRevision>, obc_app::retention::RetentionError> {
        Ok(None)
    }
    fn write_metadata(
        &mut self,
        _effect: obc_app::retention::RetentionEffect,
    ) -> Result<(), obc_app::retention::RetentionError> {
        Err(obc_app::retention::RetentionError::Unsupported)
    }
    fn expire_route(
        &mut self,
        _id: CatalogObjectId,
        _scope: obc_app::device_core::StoreRevision,
    ) -> Result<bool, CatalogError> {
        Err(CatalogError::Unsupported)
    }
}

/// The ride catalog (the Rides screen) plus the per-ride track reads its detail draws.
pub trait RideRepository {
    /// The ride catalog (paired entries, newest first), for [`App::set_rides`](obc_app::App::set_rides).
    fn catalog(&self) -> &[RideEntry];
    /// Remove the ride: `Ok(true)` = removed, `Ok(false)` = already absent, `Err` = storage failure.
    fn delete_by_id(&mut self, id: CatalogObjectId) -> Result<bool, CatalogError>;
    /// Fill the keyed ride's profile in place and return its preview from one track read.
    /// `None` = unknown/unreadable; the caller must not publish the profile on failure.
    fn fill_track(&self, id: CatalogObjectId, profile: &mut Profile) -> Option<Vec<(i32, i32)>>;
    /// Re-scan after a ride was just saved. Folder-backed simulator stores use this hook; a static
    /// in-memory catalog is a no-op.
    fn refresh(&mut self) {}
    /// Complete catalog and durable policy refresh. Legacy stores have no card authority.
    fn refresh_metadata(
        &mut self,
    ) -> Result<Option<obc_app::device_core::StoreRevision>, obc_app::retention::RetentionError> {
        self.refresh();
        Ok(None)
    }
    fn store_scope(&self) -> Option<obc_app::device_core::StoreRevision> {
        None
    }
    /// Full retention inventory when the summary catalog is capped for display.
    fn retention_inventory(&self) -> Option<&[obc_app::RideRetentionRecord]> {
        None
    }
    fn write_metadata(
        &mut self,
        _effect: obc_app::retention::RetentionEffect,
    ) -> Result<(), obc_app::retention::RetentionError> {
        Err(obc_app::retention::RetentionError::Unsupported)
    }
    fn expire_ride(
        &mut self,
        _id: CatalogObjectId,
        _scope: obc_app::device_core::StoreRevision,
    ) -> Result<bool, CatalogError> {
        Err(CatalogError::Unsupported)
    }
}

/// The open ride object the app records into while riding — one method per
/// [`RecorderEffect`](obc_app::recorder::RecorderEffect), plus the session edge that opens the
/// object.
///
/// There is no `reconcile`: the recorder lifecycle is Recorder's (#1398), and a store that
/// reconstructed it from an action plus a session id would be deciding it a second time. There is
/// no sink either: the app stages its own samples and this writes the ones it is handed (#1553).
pub trait TrackRepository {
    /// Open a ride object for `session`, to be saved under `name`. Return true only when that
    /// object is open. False leaves no object; the dispatcher retries on the next execution.
    /// Recorder opens exactly one ride at a time, so any previous object is already closed.
    fn open(&mut self, session: u32, name: Option<&str>) -> bool;

    /// Close the open ride into a durable ride object.
    ///
    /// [`RideClose::Failed`] means the ride is **still there** and Recorder re-offers the same
    /// close, so a store must not throw the bytes away on the way out; [`RideClose::Nothing`] is
    /// how a store says there was no object to close, which is over rather than owed.
    fn finalize(&mut self, stats: RideStats) -> RideClose;

    /// Delete the open ride and its journal. `false` is a **failure** and Recorder re-offers the
    /// same discard — the same rule [`finalize`](Self::finalize) follows, because a close that did
    /// not happen must not read as one that did.
    fn discard(&mut self) -> bool;

    /// Make the ride recoverable across a power loss up to this point. `false` is a failed write —
    /// Recorder owes the same checkpoint again. A store with no journal has nothing to do and says
    /// so by succeeding.
    fn checkpoint(&mut self) -> bool {
        true
    }

    /// Append one staged sample to the open ride. `false` means the medium refused it: Recorder
    /// keeps that sample and every sample behind it staged, and offers them again.
    ///
    /// A store with no log has nothing to write and says so by succeeding — the same shape
    /// [`checkpoint`](Self::checkpoint) uses, and the reason a memory store needs no arm of its own.
    fn append(&mut self, point: TrackPoint) -> bool {
        let _ = point;
        true
    }
}

/// The `.obt` trip folders that group routes (sim-only; the web demo has none, the board reads its
/// own `ObjectStore`). Every method defaults to "no trips" so a host without them plugs in the unit
/// type `()`.
pub trait TripCatalog {
    /// Delete the trip with id `id` — its backing `.obt` and nothing else. The cascade over member
    /// routes is `CatalogMachine`'s ordering (#1491) and reaches this executor as its own removals,
    /// so there is no member lookup here. `Ok(true)` = removed, `Ok(false)` = absent, `Err` = failure.
    fn delete_by_id(&mut self, id: CatalogObjectId) -> Result<bool, CatalogError> {
        let _ = id;
        Ok(false)
    }
    /// Re-scan the trip folder (a store-changed edge re-resolves the folders alongside the routes).
    fn rescan(&mut self) {}
    /// Re-feed the app's trip list ([`App::set_trips`](obc_app::App::set_trips)) — call **after** the
    /// route catalog is re-fed so the stage ids resolve.
    fn refeed(&self, app: &mut App) {
        let _ = app;
    }
}

/// The trip-less host: the web demo and any test that drives routes/rides without `.obt` grouping.
impl TripCatalog for () {}
