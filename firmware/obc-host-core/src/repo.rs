//! The narrow repository interfaces the shared host dispatcher ([`crate::HostLoop`]) drives — one
//! trait per store family the app talks to, so the *sequencing* (delete → rescan → re-feed, the
//! nav commit order, the track lifecycle) lives once in [`crate::dispatch`] and every host store —
//! the simulator's folder-backed stores and the in-memory [`MemRouteStore`](crate::MemRouteStore)
//! family — plugs in behind the same shape. Storage internals stay in the concrete stores; these
//! traits carry no `std`-vs-`no_std` assumptions of their own.
//!
//! The board deliberately does **not** implement these (its FAT/`ObjectStore` path stays async and
//! board-specific — #801 non-goal, #809 owns the board loop); the *command/event semantics* it
//! shares are pinned by protocol tests instead.

use obc_app::{App, RideSummary, TrackAction};
use obc_formats::io::SliceSource;
use obc_ports::TrackSink;
use obc_route::{Profile, RideStats, RouteSummary};

/// The route catalog + the one active route's bytes, plus the reserved nav-route commit slot the
/// router writes into. Supersedes the old `NavRouteStore` (which was only the nav-commit slice):
/// the dispatcher needs the whole delete/rescan/active surface, so it lives in one trait.
pub trait RouteRepository {
    /// The route catalog (summaries), for [`App::set_routes_with_ids`](obc_app::App::set_routes_with_ids).
    fn catalog(&self) -> &[RouteSummary];
    /// Each catalog entry's session-stable durable id, parallel to [`catalog`](RouteRepository::catalog).
    fn ids(&self) -> &[u16];
    /// Delete the route with durable id `id` (the on-device hold-to-delete). `true` = removed; the
    /// caller then re-feeds the catalog. A vanished id is a no-op.
    fn delete_by_id(&mut self, id: u16) -> bool;
    /// Persist the router's emitted OBCR as the reserved nav route (overwriting any previous plan),
    /// returning its session-stable id — or `None` on an I/O failure.
    fn write_nav_route(&mut self, bytes: &[u8]) -> Option<u16>;
    /// Make the active route match `want`, (re)reading its bytes only on a change. **Returns whether
    /// the active bytes were (re)loaded this call** — the signal [`ActiveRouteSession`](crate::ActiveRouteSession)
    /// gates its index reparse on, so a settled view never reparses.
    fn sync_active(&mut self, want: Option<usize>) -> bool;
    /// A [`ByteSource`](obc_formats::io::ByteSource) over the active route's bytes.
    fn active_source(&self) -> Option<SliceSource<'_>>;
    /// Force the active bytes to re-read on the next [`sync_active`](RouteRepository::sync_active)
    /// even under an unchanged index — a re-route rewrites the nav bytes beneath the same catalog slot.
    fn invalidate_active(&mut self);
}

/// The ride catalog (the Rides screen) plus the per-ride track reads its detail draws.
pub trait RideRepository {
    /// The ride catalog (summaries, newest first), for [`App::set_rides`](obc_app::App::set_rides).
    fn catalog(&self) -> &[RideSummary];
    /// Each catalog entry's durable id, parallel to [`catalog`](RideRepository::catalog).
    fn ids(&self) -> &[u16];
    /// Delete the ride with durable id `id` (the hold-to-delete). `true` = removed.
    fn delete_by_id(&mut self, id: u16) -> bool;
    /// The ride's recorded-track elevation [`Profile`] — the Ride detail's band fill (answers the
    /// [`LoadRideTrack`](obc_app::HostCommand::LoadRideTrack) cue). `None` = unknown/unreadable.
    fn profile_by_id(&self, id: u16) -> Option<Profile>;
    /// The ride's decimated recorded-track shape polyline (the detail's track page). Empty =
    /// unknown/unreadable.
    fn preview_by_id(&self, id: u16) -> Vec<(i32, i32)>;
    /// Re-scan the catalog after a ride was just saved (a folder store picks up the new `RD{id}.ORD`);
    /// a static in-memory catalog is a no-op.
    fn refresh(&mut self) {}
}

/// The open ride log the app records into while riding — reconciled to the app's tracking intent
/// each pass, exposing its [`TrackSink`] when recording.
pub trait TrackRepository {
    /// Reconcile the open log to the app's tracking intent: finalise/abandon the current log for the
    /// drained `action`, then (re)open a log to match `session`. `name`/`stats` are the save
    /// filename + ride totals for a `Save` (a memory store ignores both).
    fn reconcile(
        &mut self,
        action: Option<TrackAction>,
        session: Option<u32>,
        name: Option<&str>,
        stats: Option<RideStats>,
    );
    /// The [`TrackSink`] for the open log, or `None` when nothing is recording.
    fn sink(&mut self) -> Option<&mut dyn TrackSink>;
}

/// The `.obt` trip folders that group routes (sim-only; the web demo has none, the board wires its
/// own `ObjectStore` cascade). Every method defaults to "no trips" so a host without them plugs in
/// the unit type `()`.
pub trait TripCatalog {
    /// The member route ids of the trip with id `id`, for the cascade delete (delete the trip *and*
    /// its member routes). Empty for an unknown id / a trip-less host.
    fn member_route_ids(&self, id: u16) -> Vec<u16> {
        let _ = id;
        Vec::new()
    }
    /// Delete the trip with id `id` (its backing `.obt` only — the cascade over member routes is the
    /// dispatcher's composition). `true` = removed.
    fn delete_by_id(&mut self, id: u16) -> bool {
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
