//! The narrow repository interfaces the shared typed executor ([`crate::HostLoop`]) drives — one
//! trait per store family the app talks to, so the *sequencing* (delete → rescan → re-feed, the
//! nav commit order, the track lifecycle) lives once in [`crate::dispatch`] and every host store —
//! the simulator's folder-backed stores and the in-memory [`MemRouteStore`](crate::MemRouteStore)
//! family — plugs in behind the same shape. Storage internals stay in the concrete stores; these
//! traits carry no `std`-vs-`no_std` assumptions of their own.
//!
//! The board deliberately does **not** implement these (its FAT/`ObjectStore` path stays async and
//! board-specific — #801 non-goal, #809 owns the board loop); the *command/event semantics* it
//! shares are pinned by protocol tests instead.

use obc_app::recorder::RideClose;
use obc_app::{App, CatalogObjectId, RideSummary, RouteRetentionMeta};
use obc_formats::io::SliceSource;
use obc_ports::TrackPoint;
use obc_route::{Profile, RideStats, RouteSummary};

/// The route catalog + the one active route's bytes, plus the reserved nav-route commit slot the
/// router writes into. Supersedes the old `NavRouteStore` (which was only the nav-commit slice):
/// the dispatcher needs the whole delete/rescan/active surface, so it lives in one trait.
pub trait RouteRepository {
    /// The route catalog (summaries), for [`App::set_routes_with_ids`](obc_app::App::set_routes_with_ids).
    fn catalog(&self) -> &[RouteSummary];
    /// Each catalog entry's session-stable durable id, parallel to [`catalog`](RouteRepository::catalog).
    fn ids(&self) -> &[CatalogObjectId];
    /// Delete the route with durable id `id` (the on-device hold-to-delete). `true` = removed; the
    /// caller then re-feeds the catalog. A vanished id is a no-op.
    fn delete_by_id(&mut self, id: CatalogObjectId) -> bool;
    /// Persist the router's emitted OBCR as the reserved nav route (overwriting any previous plan),
    /// returning its session-stable id — or `None` on an I/O failure.
    fn write_nav_route(&mut self, bytes: &[u8]) -> Option<CatalogObjectId>;
    /// Make the active route match `want`, (re)reading its bytes only on a change. **Returns whether
    /// the active bytes were (re)loaded this call** — the signal [`ActiveRouteSession`](crate::ActiveRouteSession)
    /// gates its index reparse on, so a settled view never reparses.
    fn sync_active(&mut self, want: Option<usize>) -> bool;
    /// A [`ByteSource`](obc_formats::io::ByteSource) over the active route's bytes.
    fn active_source(&self) -> Option<SliceSource<'_>>;
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
    /// Stamp route `id`'s `last_used` to `utc` in the retention sidecar — the sweep's clock-start /
    /// active re-stamp, and the once-per-activation stamp
    /// (a `RetentionEffect::WriteRouteMetadata`). Default no-op (a retention-less
    /// host has no sidecar).
    fn stamp_route_used(&mut self, id: CatalogObjectId, utc: u32) {
        let _ = (id, utc);
    }
}

/// The ride catalog (the Rides screen) plus the per-ride track reads its detail draws.
pub trait RideRepository {
    /// The ride catalog (summaries, newest first), for [`App::set_rides`](obc_app::App::set_rides).
    fn catalog(&self) -> &[RideSummary];
    /// Each catalog entry's durable id, parallel to [`catalog`](RideRepository::catalog).
    fn ids(&self) -> &[CatalogObjectId];
    /// Delete the ride with durable id `id` (the hold-to-delete). `true` = removed.
    fn delete_by_id(&mut self, id: CatalogObjectId) -> bool;
    /// The ride's recorded-track elevation [`Profile`] — the Ride detail's band fill (answers the
    /// keyed ride-track need). `None` = unknown/unreadable.
    fn profile_by_id(&self, id: CatalogObjectId) -> Option<Profile>;
    /// The ride's decimated recorded-track shape polyline (the detail's track page). Empty =
    /// unknown/unreadable.
    fn preview_by_id(&self, id: CatalogObjectId) -> Vec<(i32, i32)>;
    /// Re-scan after a ride was just saved. Folder-backed simulator stores use this hook; a static
    /// in-memory catalog is a no-op.
    fn refresh(&mut self) {}
    /// Stamp ride `id`'s `synced_at` to `utc` in the synced sidecar (epic #638, S3) — the sweep's
    /// legacy synced-without-stamp countdown start
    /// (a `RetentionEffect::WriteRideMetadata`). Default no-op.
    fn stamp_synced_at(&mut self, id: CatalogObjectId, utc: u32) {
        let _ = (id, utc);
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
    /// Open a ride object for `session`, to be saved under `name`. Called on the session edge —
    /// Recorder opens exactly one ride at a time, so any previous object is already closed.
    fn open(&mut self, session: u32, name: Option<&str>);

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
    /// so there is no member lookup here. `true` = removed.
    fn delete_by_id(&mut self, id: CatalogObjectId) -> bool {
        let _ = id;
        false
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
