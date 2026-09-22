//! [`CatalogState`] — the resident route, ride and trip catalogs, keyed by durable object ids.
//!
//! One component owns every id-to-summary pairing and every piece of state keyed by catalog
//! identity: the route and ride catalogs with their durable ids, the trip folders resolving stage
//! ids against the route catalog, and the executor-filled derived targets held under the
//! [`derived`](crate::device_core::derived) keys — a durable identity plus a source and a view
//! revision, so no cache key has to be walked across a live rescan.
//!
//! Ride rows own their durable identity and summary together. Route summaries stay contiguous with
//! a parallel id column and are exposed through [`RouteEntry`].
//!
//! `App` stays the composition root: screen-stack remaps and `Activity` key remaps live there, and
//! this component never sees a `Screen`.

use obc_route::Profile;

use crate::app::NAV_PREVIEW_MAX;
use crate::device_core::derived::{DerivedInput, NavPreviewKey, RideTrackKey};
use crate::device_core::Revision;
use crate::placement::define_placement_constructors;
use crate::ride::{RideCatalog, RideEntry, UI_RIDES_CAP};
use crate::route::{Catalog, RouteSummary, MAX_ROUTES};
use crate::trip::{TripInput, TripSummary, Trips, MAX_TRIPS};
use crate::CatalogObjectId;

/// One route-catalog entry: the durable object id and its summary, handed out together so the
/// pairing is a type, not a convention.
#[derive(Debug, Clone, Copy)]
pub struct RouteEntry<'a> {
    /// The route's durable object id, which survives a live rescan.
    pub id: CatalogObjectId,
    pub summary: &'a RouteSummary,
}

/// The snapshot of a catalog's ids before a replacement, which
/// [`CatalogState::remap_route`] and [`CatalogState::remap_ride`] resolve an old index through.
/// Returned by the `replace_*` methods so `App` can re-point the screen stack and Navigator keys
/// with the exact mapping the component used.
pub(crate) type OldRouteIds = heapless::Vec<CatalogObjectId, MAX_ROUTES>;
pub(crate) type OldRideIds = heapless::Vec<CatalogObjectId, UI_RIDES_CAP>;

/// The resident catalogs and identity-keyed view caches. See the module docs.
pub(crate) struct CatalogState {
    routes: Catalog,
    /// Each route's durable object id, pairwise with [`routes`](CatalogState::routes) and only ever
    /// written in lock step with it.
    route_ids: heapless::Vec<CatalogObjectId, MAX_ROUTES>,
    /// Grouped-route folders resolving their stage route ids against
    /// [`route_ids`](CatalogState::route_ids). Re-resolved on every route replacement, so a route
    /// that appeared or vanished re-files.
    trips: Trips,
    rides: RideCatalog,
    /// The viewed ride's recorded-track elevation profile: the Ride detail's band source,
    /// host-filled once per detail entry.
    ride_profile: Profile,
    /// Whether [`ride_profile`](CatalogState::ride_profile) holds a successful host answer. Kept
    /// separate so the board can stream into the resident buffer without returning a ~5 KiB value
    /// through its task frame.
    ride_profile_present: bool,
    /// The key [`ride_profile`](CatalogState::ride_profile) was answered for. A failed fill parks
    /// the same key with `present == false`, so a dead file is answered once rather than
    /// re-streamed every pass.
    ride_profile_for: Option<RideTrackKey>,
    /// The viewed ride's decimated recorded-track shape, host-filled in the same drain as the
    /// profile.
    ride_preview: heapless::Vec<(i32, i32), NAV_PREVIEW_MAX>,
    ride_preview_for: Option<RideTrackKey>,
    /// The Route overview's decimated route shape, host-decimated and handed in through
    /// [`accept_nav_preview`](CatalogState::accept_nav_preview).
    nav_preview: heapless::Vec<(i32, i32), NAV_PREVIEW_MAX>,
    /// The staleness key for [`nav_preview`](CatalogState::nav_preview): the render gates on it, so
    /// an old plan's shape can never draw under a different route or under fresh geometry.
    nav_preview_route: Option<NavPreviewKey>,
    /// The revision of the object bytes this component knows about, bumped by
    /// [`note_commit`](CatalogState::note_commit). It is the `source` half of every derived key,
    /// and the reason a route upload that replaces a stored route cannot leave its old preview
    /// standing.
    ///
    /// One store-wide counter rather than one per namespace: a re-commit is rare, and the only cost
    /// of the coarser key is one extra derived read when an unrelated route is replaced while a
    /// ride detail is open.
    source_revision: Revision,
    /// The ride-track view generation, bumped when an in-place fill starts, so an abandoned fill
    /// leaves the need up instead of a half-written buffer answered.
    ride_track_view: Revision,
    /// The nav-preview view generation, bumped by
    /// [`invalidate_nav_preview`](CatalogState::invalidate_nav_preview) so every committed plan
    /// starts preview-less even when the route identity and its bytes are unchanged.
    nav_preview_view: Revision,
    /// The planned-but-uncommitted detour's decimated shape, drawn by the Detour preview screen
    /// over the still-active original route. Cleared on commit, cancel, or route change.
    detour_preview: heapless::Vec<(i32, i32), NAV_PREVIEW_MAX>,
    /// The staleness key for [`detour_preview`](CatalogState::detour_preview): a route swap or
    /// rescan mid-preview blanks the overlay rather than drawing a stale detour over different
    /// geometry.
    detour_preview_route: Option<usize>,
    /// One operation is in flight at a time, so one token source is all the domain needs.
    ops: crate::device_core::TokenSource<crate::device_core::CatalogTag>,
    /// An admitted [`CatalogIntent`] that has not become an effect yet. Capacity one: later work
    /// stays with whoever asked for it, where it can still be superseded or cancelled.
    ///
    /// A [`DeleteTrip`](CatalogIntent::DeleteTrip) stays here for the whole cascade rather than one
    /// effect, so the one slot that delays a second delete also delays one behind a cascade.
    pending: Option<CatalogIntent>,
    /// How far the trip cascade has walked: the stage ordinal the next member removal takes, or
    /// `None` when no cascade is running. Two bytes is the whole resident cost of the cascade,
    /// because the member ids are already resident in
    /// [`trips`](CatalogState::trips)`[..].stage_ids`.
    cascade: Option<u8>,
    /// Whether an effect is out with the executor. Its outcome clears this, which is what lets the
    /// next intent go out.
    in_flight: bool,
    cleanup_running: bool,
    /// The resident catalogs are behind the store, and a re-read has not gone out yet. Armed by
    /// [`note_store_moved`](CatalogState::note_store_moved), by a completed removal, and by a read
    /// the store could not answer; spent by [`next_effect_at`](CatalogState::next_effect_at) once
    /// nothing is pending.
    ///
    /// A bit, not a counter, and that is the whole coalescing rule: a delete that also moves the
    /// store arms the same bit twice and costs one read, not two.
    refresh_owed: bool,
    pub(crate) loaded_scope: Option<StoreRevision>,
    pub(crate) remount_required: bool,
    read_retry_at: Option<u32>,
}

impl CatalogState {
    define_placement_constructors!(
        /// Empty catalogs, nothing cached: the boot state.
        pub(crate) fn new();
        /// Initialize `slot` in place to the [`new`](CatalogState::new) state. The catalogs are
        /// several KB, so the firmware boots through this path and never forms a by-value
        /// `CatalogState` on the stack.
        pub(crate) unsafe fn init_in_place;
        fields {
            routes: Catalog::new(),
            route_ids: heapless::Vec::new(),
            trips: Trips::new(),
            rides: RideCatalog::new(),
            ride_profile: Profile::EMPTY,
            ride_profile_present: false,
            ride_profile_for: None,
            ride_preview: heapless::Vec::new(),
            ride_preview_for: None,
            nav_preview: heapless::Vec::new(),
            nav_preview_route: None,
            source_revision: Revision::ZERO,
            ride_track_view: Revision::ZERO,
            nav_preview_view: Revision::ZERO,
            detour_preview: heapless::Vec::new(),
            detour_preview_route: None,
            ops: crate::device_core::TokenSource::new(),
            pending: None,
            cascade: None,
            in_flight: false,
            cleanup_running: false,
            refresh_owed: false,
            loaded_scope: None,
            remount_required: false,
            read_retry_at: None,
        }
    );

    pub(crate) fn routes(&self) -> &[RouteSummary] {
        &self.routes
    }

    /// Each route's durable id, pairwise with [`routes`](CatalogState::routes).
    pub(crate) fn route_ids(&self) -> &[CatalogObjectId] {
        &self.route_ids
    }

    /// The durable id at catalog index `idx`, or `None` out of range. A vanished subject resolves
    /// to nothing.
    pub(crate) fn route_id_at(&self, idx: usize) -> Option<CatalogObjectId> {
        self.route_ids.get(idx).copied()
    }

    pub(crate) fn route_index_of(&self, id: CatalogObjectId) -> Option<usize> {
        self.route_ids.iter().position(|&x| x == id)
    }

    pub(crate) fn route_len(&self) -> usize {
        self.routes.len()
    }

    /// Replace the route catalog from the host's store (`ids` pairwise with `summaries`; entries
    /// past [`MAX_ROUTES`] are ignored), re-resolve the trip folders against the new ids, and
    /// return the old id column so the caller can remap every held index by identity
    /// ([`remap_route`](CatalogState::remap_route)).
    pub(crate) fn replace_routes(&mut self, summaries: &[RouteSummary], ids: &[CatalogObjectId]) -> OldRouteIds {
        let old_ids = self.route_ids.clone();
        self.routes.clear();
        self.route_ids.clear();
        for (s, &id) in summaries.iter().zip(ids).take(MAX_ROUTES) {
            let _ = self.routes.push(s.clone());
            let _ = self.route_ids.push(id);
        }
        // Trips resolve stage ids into catalog indices, so a catalog replacement re-points them: a
        // route that appeared re-files, one that vanished dangles. Re-resolved here, before the
        // caller's stack walk, so the Route menu's remap sees the regrouped folders.
        for t in self.trips.iter_mut() {
            t.reresolve(&self.routes, &self.route_ids);
        }
        old_ids
    }

    /// Old route index to new route index by durable identity, or `None` when that route vanished.
    /// Every held index — `active_route`, cache keys, open screens — follows a rescan through this.
    pub(crate) fn remap_route(&self, old_ids: &[CatalogObjectId], idx: usize) -> Option<usize> {
        let id = *old_ids.get(idx)?;
        self.route_index_of(id)
    }

    /// Replace the resident trip catalog, resolving each trip's stage ids against the current route
    /// catalog. Trips past [`MAX_TRIPS`] are ignored. Call after the routes are set so the stage
    /// ids resolve; a later [`replace_routes`](CatalogState::replace_routes) re-resolves them.
    pub(crate) fn set_trips(&mut self, trips: &[TripInput]) {
        self.trips.clear();
        for input in trips.iter().take(MAX_TRIPS) {
            let _ = self.trips.push(TripSummary::resolve(input, &self.routes, &self.route_ids));
        }
    }

    pub(crate) fn trips(&self) -> &[TripSummary] {
        &self.trips
    }

    /// Whether the route at catalog index `idx` is filed into some trip. A filed route shows only
    /// inside its folder.
    pub(crate) fn route_filed(&self, idx: usize) -> bool {
        let i = idx as u16;
        self.trips.iter().any(|t| t.stage_indices.contains(&i))
    }

    pub(crate) fn rides(&self) -> &[RideEntry] {
        &self.rides
    }
    /// Overlay a fully validated proof during a catalog refresh, in the visible catalog.
    pub(crate) fn set_ride_archive_proof(&mut self, id: CatalogObjectId, timestamp: u32) {
        if let Some(ride) = self.rides.iter_mut().find(|ride| ride.id == id) {
            ride.summary.synced = true;
            ride.summary.synced_at_utc = timestamp;
        }
    }

    pub(crate) fn ride_entry(&self, idx: usize) -> Option<&RideEntry> {
        self.rides.get(idx)
    }

    pub(crate) fn ride_len(&self) -> usize {
        self.rides.len()
    }

    pub(crate) fn replace_rides(&mut self, entries: &[RideEntry]) -> OldRideIds {
        let old_ids = self.rides.iter().map(|ride| ride.id).collect();
        self.rides.clear();
        for entry in entries.iter().take(UI_RIDES_CAP) {
            let _ = self.rides.push(entry.clone());
        }
        // The view caches need no remap: their keys name a durable ride identity, so a surviving
        // ride keeps its answer and a vanished one stops matching any key the need can produce.
        old_ids
    }

    /// Old ride index to new ride index by durable identity, the twin of
    /// [`remap_route`](CatalogState::remap_route).
    pub(crate) fn remap_ride(&self, old_ids: &[CatalogObjectId], idx: usize) -> Option<usize> {
        let id = *old_ids.get(idx)?;
        self.rides.iter().position(|ride| ride.id == id)
    }

    // Keyed derived data. Two derived reads, two
    // [`DerivedNeeds`](crate::device_core::derived::DerivedNeeds) slots, and one rule for both: the
    // answer is stored under the key the need carried, and every read compares that key with the
    // key the need would carry now. Nothing is remapped and nothing is invalidated on rescan — a
    // subject change, fresh bytes or an explicit invalidate produces a different key, and the old
    // answer becomes unreachable in the same instant.

    /// The derived ride-track key for the ride at catalog index `viewed_ride`, or `None` when no
    /// detail is open or its subject vanished. An answer must bring the same key back.
    pub(crate) fn ride_track_key(&self, viewed_ride: Option<usize>) -> Option<RideTrackKey> {
        let ride = self.rides.get(viewed_ride?)?.id;
        Some(RideTrackKey { ride, source: self.source_revision, view: self.ride_track_view })
    }

    /// The derived nav-preview key for the route at catalog index `active_route`, the twin of
    /// [`ride_track_key`](Self::ride_track_key).
    pub(crate) fn nav_preview_key(&self, active_route: Option<usize>, assistant: bool) -> Option<NavPreviewKey> {
        let route = *self.route_ids.get(active_route?)?;
        Some(NavPreviewKey { assistant, route, source: self.source_revision, view: self.nav_preview_view })
    }

    /// Whether the ride-track need for `key` is already answered. A recorded failure counts, so a
    /// dead file is read once rather than on every pass.
    ///
    /// The profile alone is the authoritative answer, not the profile and the preview: every host
    /// fills both targets in one drain of the same read, and requiring both would make a host with
    /// no track shape to hand in re-fire the read forever.
    pub(crate) fn ride_track_answered(&self, key: RideTrackKey) -> bool {
        self.ride_profile_for == Some(key)
    }

    pub(crate) fn nav_preview_answered(&self, key: NavPreviewKey) -> bool {
        self.nav_preview_route == Some(key)
    }

    /// Note that something committed new bytes over a durable identity, such as an upload that
    /// replaced a stored object. Every derived key moves with it, so an answer produced from the
    /// previous bytes stops matching. This is the one case identity alone cannot catch.
    pub(crate) fn note_commit(&mut self) {
        self.source_revision = self.source_revision.next();
    }

    /// Accept a keyed ride-profile answer, and report whether it was accepted. A stale key changes
    /// nothing: the payload is dropped and the need stays up, so a fill that finished after the
    /// rider moved on cannot land on the ride they are looking at now.
    ///
    /// The refusal only bites once an executor carries the key it was asked with. Today the caller
    /// derives `current` from the live subject and hands the same value as `input.key`, so it can
    /// never refuse, and a late answer can still be misattributed.
    pub(crate) fn accept_ride_profile(
        &mut self,
        current: Option<RideTrackKey>,
        input: DerivedInput<RideTrackKey>,
        profile: Option<Profile>,
    ) -> bool {
        if current != Some(input.key) {
            return false;
        }
        // A `Filled` result with no payload is an in-place fill: the buffer is already written.
        if let Some(profile) = profile {
            self.ride_profile = profile;
        }
        self.ride_profile_present = input.result.is_filled();
        self.ride_profile_for = Some(input.key);
        true
    }

    /// Borrow the one resident profile buffer for an in-place fill, invalidating the ride-track
    /// view first: until the matching accept lands, the need carries a new key and re-emits, so an
    /// abandoned fill leaves a need up rather than a half-written buffer marked answered.
    pub(crate) fn begin_ride_profile_fill(&mut self) -> &mut Profile {
        self.ride_profile_present = false;
        self.ride_track_view = self.ride_track_view.next();
        &mut self.ride_profile
    }

    /// Accept a keyed ride-preview answer, truncated to [`NAV_PREVIEW_MAX`] points, under the same
    /// staleness rule as the profile.
    pub(crate) fn accept_ride_preview(
        &mut self,
        current: Option<RideTrackKey>,
        input: DerivedInput<RideTrackKey>,
        pts: &[(i32, i32)],
    ) -> bool {
        if current != Some(input.key) {
            return false;
        }
        self.ride_preview.clear();
        if input.result.is_filled() {
            for &p in pts.iter().take(NAV_PREVIEW_MAX) {
                let _ = self.ride_preview.push(p);
            }
        }
        self.ride_preview_for = Some(input.key);
        true
    }

    /// The resident ride profile only if it was answered for `key`: the buffer is reachable through
    /// the exact key it was filled for and no other.
    pub(crate) fn ride_profile_for(&self, key: Option<RideTrackKey>) -> Option<&Profile> {
        (self.ride_profile_present && key.is_some() && self.ride_profile_for == key).then_some(&self.ride_profile)
    }

    /// The ride-shape preview for `key`, or the empty slice when missing or stale. The screens draw
    /// whatever this hands them, so a stale shape is unreachable.
    pub(crate) fn ride_preview_for(&self, key: Option<RideTrackKey>) -> &[(i32, i32)] {
        if key.is_some() && self.ride_preview_for == key {
            &self.ride_preview
        } else {
            &[]
        }
    }

    /// Release the ride profile and preview once they stop matching the live key, because the
    /// detail exited or moved subjects. The key gate already makes them unreachable; dropping the
    /// keys is what lets the need re-fire when the rider comes back.
    pub(crate) fn drop_stale_ride_views(&mut self, key: Option<RideTrackKey>) {
        if self.ride_profile_for != key {
            self.ride_profile_present = false;
            self.ride_profile_for = None;
        }
        if self.ride_preview_for != key {
            self.ride_preview.clear();
            self.ride_preview_for = None;
        }
    }

    /// Accept a keyed nav-preview answer: the previewed route's decimated shape.
    pub(crate) fn accept_nav_preview(
        &mut self,
        current: Option<NavPreviewKey>,
        input: DerivedInput<NavPreviewKey>,
        pts: &[(i32, i32)],
    ) -> bool {
        if current != Some(input.key) {
            return false;
        }
        self.nav_preview.clear();
        if input.result.is_filled() {
            for &p in pts.iter().take(NAV_PREVIEW_MAX) {
                let _ = self.nav_preview.push(p);
            }
        }
        self.nav_preview_route = Some(input.key);
        true
    }

    /// The route-shape preview for `key`, or the empty slice when missing or stale.
    pub(crate) fn nav_preview_for(&self, key: Option<NavPreviewKey>) -> &[(i32, i32)] {
        if key.is_some() && self.nav_preview_route == key {
            &self.nav_preview
        } else {
            &[]
        }
    }

    /// Invalidate the nav preview: drop it and bump the view generation, so every committed plan
    /// starts preview-less even when the route identity and its bytes are unchanged. The bump is
    /// what makes this an invalidate rather than a clear a late answer could undo.
    pub(crate) fn invalidate_nav_preview(&mut self) {
        self.nav_preview.clear();
        self.nav_preview_route = None;
        self.nav_preview_view = self.nav_preview_view.next();
    }

    /// Hand in a planned detour's decimated polyline, keyed to the route it was planned against.
    /// [`detour_preview_for`](CatalogState::detour_preview_for) gates on the same key.
    pub(crate) fn set_detour_preview(&mut self, pts: &[(i32, i32)], active_route: Option<usize>) {
        self.detour_preview.clear();
        for &p in pts.iter().take(NAV_PREVIEW_MAX) {
            let _ = self.detour_preview.push(p);
        }
        self.detour_preview_route = active_route;
    }

    /// The detour-preview polyline for `active_route`, or the empty slice when missing or stale.
    pub(crate) fn detour_preview_for(&self, active_route: Option<usize>) -> &[(i32, i32)] {
        if self.detour_preview_route.is_some() && self.detour_preview_route == active_route {
            &self.detour_preview
        } else {
            &[]
        }
    }

    /// Clear the detour preview and its key. A commit, cancel or failure ends the preview.
    pub(crate) fn clear_detour_preview(&mut self) {
        self.detour_preview.clear();
        self.detour_preview_route = None;
    }
}

// The catalog domain protocol. This domain owns every ordering: delete-then-refresh, the trip
// cascade's member-then-folder order, and the identity remap a refresh implies. The store executor
// is left with two operations, read the catalog and remove an object, and no say in what either of
// them means.
//
// Every re-read is ordered here. Three events say the resident catalogs are behind the store — the
// store moved underneath us, a removal completed, a read failed — and all three arm one bit. No
// executor decides whether, when, or how many times a refresh happens.
//
// Bulk stays out. A catalog read fills the resident catalogs through their existing feeders, and
// the outcome reports only that the operation is over.

use crate::device_core::{CatalogTag, OperationToken, StoreRevision};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogIntent {
    RemoveReview {
        source: obc_formats::obcr::RouteSourceKey,
    },
    CleanupRoutes {
        before_utc: u32,
        store: crate::device_core::StoreIdentity,
    },

    DeleteRoute {
        id: CatalogObjectId,
    },
    DeleteRide {
        id: CatalogObjectId,
    },
    /// Delete one trip and its member routes: the cascade, whose order the domain owns.
    DeleteTrip {
        id: CatalogObjectId,
    },
}

/// The family selected by catalog policy, retained through physical deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CatalogObjectKind {
    Route,
    Ride,
    Trip,
}

/// One bounded physical catalog operation, carrying the [`OperationToken`] the domain issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogEffect {
    RemoveReview {
        token: OperationToken<CatalogTag>,
        source: obc_formats::obcr::RouteSourceKey,
    },
    CleanupRoute {
        token: OperationToken<CatalogTag>,
        before_utc: u32,
        store: crate::device_core::StoreIdentity,
    },
    /// Re-read the object store into the resident catalogs.
    ReadCatalog {
        token: OperationToken<CatalogTag>,
    },
    RemoveObject {
        token: OperationToken<CatalogTag>,
        object: CatalogObjectId,
        kind: CatalogObjectKind,
    },
}

impl CatalogEffect {
    pub fn token(&self) -> OperationToken<CatalogTag> {
        match self {
            CatalogEffect::CleanupRoute { token, .. }
            | CatalogEffect::ReadCatalog { token }
            | CatalogEffect::RemoveObject { token, .. }
            | CatalogEffect::RemoveReview { token, .. } => *token,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogError {
    Stale,
    RemountRequired,
    Unsupported,
    Unreadable,
    /// The store refused or failed the removal. A missing object is not this; see
    /// [`ObjectRemoved`](CatalogOutcome::ObjectRemoved)'s `existed`.
    RemoveFailed,
}

/// The result of one [`CatalogEffect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogOutcome {
    ReviewRemoved {
        token: OperationToken<CatalogTag>,
        source: obc_formats::obcr::RouteSourceKey,
    },
    CleanupFinished {
        token: OperationToken<CatalogTag>,
    },
    /// The catalogs were re-read. No revision: that arrives as an external fact, and all a read
    /// owes back is that the operation is over.
    CatalogRead {
        token: OperationToken<CatalogTag>,
        scope: Option<StoreRevision>,
    },
    /// `object` is gone from the store. `existed` is `false` when it was already absent, which is a
    /// success for the cascade — the goal state holds — and must not read as a failure.
    ObjectRemoved {
        token: OperationToken<CatalogTag>,
        object: CatalogObjectId,
        existed: bool,
    },
    Failed {
        token: OperationToken<CatalogTag>,
        error: CatalogError,
    },
    /// The executor abandoned the operation without completing it.
    Cancelled {
        token: OperationToken<CatalogTag>,
    },
}

impl CatalogOutcome {
    pub fn token(&self) -> OperationToken<CatalogTag> {
        match self {
            CatalogOutcome::CleanupFinished { token }
            | CatalogOutcome::CatalogRead { token, .. }
            | CatalogOutcome::ObjectRemoved { token, .. }
            | CatalogOutcome::ReviewRemoved { token, .. }
            | CatalogOutcome::Failed { token, .. }
            | CatalogOutcome::Cancelled { token } => *token,
        }
    }
}

/// The catalog domain's operation seam: admit an intent, issue the effects it implies, and accept
/// each answer.
///
/// It owns its [`OperationToken`] and how many operations may be in flight, plus the one ordering
/// no single bounded operation can express: the trip cascade, member routes first, folder last.
impl CatalogState {
    pub(crate) fn can_admit_intent(&self) -> bool {
        self.pending.is_none() && !self.in_flight && !self.refresh_owed && !self.remount_required
    }

    /// Admit `intent`, or refuse it and hand it back.
    ///
    /// There is one refusal, and it is backpressure rather than failure: something is already in
    /// the slot, either an intent waiting to become an effect or a cascade still walking. Its
    /// producer keeps it, so a busy catalog delays a delete and never loses one.
    pub(crate) fn admit_intent(
        &mut self,
        intent: CatalogIntent,
    ) -> Result<(), crate::device_core::SlotFull<CatalogIntent>> {
        if self.pending.is_some() {
            return Err(crate::device_core::SlotFull { rejected: intent });
        }
        self.pending = Some(intent);
        Ok(())
    }

    /// The next bounded catalog operation, or `None` while one is already in flight and nothing is
    /// admitted or owed.
    ///
    /// Deletions come first and the owed re-read last, by construction rather than by a priority
    /// list: an admitted intent is a rider's deletion request, and the re-read is only reached when
    /// there is none. It is also why the re-read never occupies the one intent slot, where a second
    /// copy of it would be a second read.
    ///
    /// The admitted intent is taken before the match, so no arm can leave the domain holding an
    /// intent it has already decided about. A cascade is the one arm that puts it back, and it
    /// advances [`cascade`](CatalogState::cascade) every time it does, so the walk reaches the
    /// folder or stops at its first failure.
    #[cfg(test)]
    pub(crate) fn next_effect(&mut self) -> Option<CatalogEffect> {
        self.next_effect_at(0)
    }

    pub(crate) fn change_store(&mut self) {
        self.ops.invalidate();
        self.in_flight = false;
        self.cleanup_running = false;
        self.pending = None;
        self.cascade = None;
        self.loaded_scope = None;
        self.read_retry_at = None;
    }

    pub(crate) fn cleanup_running(&self) -> bool {
        self.cleanup_running
    }

    pub(crate) fn accepts(&self, outcome: CatalogOutcome) -> bool {
        self.ops.is_current(outcome.token())
    }

    pub(crate) fn defer_read(&mut self, now_ms: u32) {
        self.read_retry_at = Some(now_ms.wrapping_add(30_000));
    }

    pub(crate) fn next_effect_at(&mut self, now_ms: u32) -> Option<CatalogEffect> {
        if self.remount_required || self.in_flight {
            return None;
        }
        let Some(intent) = self.pending.take() else {
            if self.read_retry_at.is_some_and(|at| now_ms.wrapping_sub(at) >= (1 << 31)) {
                return None;
            }
            self.read_retry_at = None;
            if !core::mem::take(&mut self.refresh_owed) {
                return None;
            }
            self.in_flight = true;
            self.loaded_scope = None;
            return Some(CatalogEffect::ReadCatalog { token: self.ops.issue() });
        };
        let effect = match intent {
            CatalogIntent::RemoveReview { source } => CatalogEffect::RemoveReview { token: self.ops.issue(), source },
            CatalogIntent::CleanupRoutes { before_utc, store } => {
                self.cleanup_running = true;
                self.pending = Some(intent);
                CatalogEffect::CleanupRoute { token: self.ops.issue(), before_utc, store }
            }
            CatalogIntent::DeleteRoute { id } => {
                CatalogEffect::RemoveObject { token: self.ops.issue(), object: id, kind: CatalogObjectKind::Route }
            }
            CatalogIntent::DeleteRide { id } => {
                CatalogEffect::RemoveObject { token: self.ops.issue(), object: id, kind: CatalogObjectKind::Ride }
            }
            // The cascade, one member per operation. The trip's stage ids are already resident and
            // the `.obt` is untouched until the last step, so ordinal `n` names the same member on
            // every pass: the domain needs a cursor, not a member buffer.
            CatalogIntent::DeleteTrip { id } => {
                let ordinal = self.cascade.unwrap_or(0);
                match self.trip_member(id, ordinal) {
                    Some(member) => {
                        self.cascade = Some(ordinal.saturating_add(1));
                        self.pending = Some(intent); // the folder is still owed
                        CatalogEffect::RemoveObject {
                            token: self.ops.issue(),
                            object: member,
                            kind: CatalogObjectKind::Route,
                        }
                    }
                    // Every member has had its turn: the folder itself is the last removal, and
                    // taking the intent above is what ends the cascade.
                    None => {
                        self.cascade = None;
                        CatalogEffect::RemoveObject {
                            token: self.ops.issue(),
                            object: id,
                            kind: CatalogObjectKind::Trip,
                        }
                    }
                }
            }
        };
        self.in_flight = true;
        Some(effect)
    }

    /// Whether the admitted intent would remove `object` — the route itself, or a trip folder's
    /// member. Automatic cleanup is excluded: it skips a protected route on its own.
    pub(crate) fn pending_removes(&self, object: CatalogObjectId) -> bool {
        match self.pending {
            Some(CatalogIntent::DeleteRoute { id }) | Some(CatalogIntent::DeleteRide { id }) => id == object,
            Some(CatalogIntent::DeleteTrip { id }) => {
                id == object || self.trips.iter().any(|trip| trip.id == id && trip.stage_ids.contains(&object))
            }
            _ => false,
        }
    }

    /// The member route id at stage `ordinal` of the resident trip `trip`, or `None` past its last
    /// stage. A trip that is not resident also answers `None`, which ends the walk at the folder.
    fn trip_member(&self, trip: CatalogObjectId, ordinal: u8) -> Option<CatalogObjectId> {
        let trip = self.trips.iter().find(|t| t.id == trip)?;
        trip.stage_ids.get(usize::from(ordinal)).copied()
    }

    pub(crate) fn apply_outcome(&mut self, outcome: CatalogOutcome) -> Option<CatalogObjectId> {
        if !self.ops.is_current(outcome.token()) {
            return None;
        }
        self.ops.invalidate(); // terminal: a duplicate of this answer is no longer current
        self.in_flight = false;
        if core::mem::take(&mut self.cleanup_running) && !matches!(outcome, CatalogOutcome::ObjectRemoved { .. }) {
            self.pending = None;
        }
        if matches!(outcome, CatalogOutcome::Failed { .. })
            && self.cascade.is_some()
            && matches!(self.pending, Some(CatalogIntent::DeleteTrip { .. }))
        {
            self.pending = None;
            self.cascade = None;
        }
        match outcome {
            CatalogOutcome::CleanupFinished { .. } => {
                self.refresh_owed = true;
                None
            }
            CatalogOutcome::Failed { error: CatalogError::RemountRequired, .. } => {
                self.remount_required = true;
                self.loaded_scope = None;
                self.refresh_owed = false;
                None
            }
            CatalogOutcome::CatalogRead { scope, .. } => {
                self.loaded_scope = scope;
                self.read_retry_at = None;
                None
            }
            CatalogOutcome::ReviewRemoved { .. } => {
                self.loaded_scope = None;
                self.refresh_owed = true;
                None
            }
            CatalogOutcome::ObjectRemoved { object, .. } => {
                self.loaded_scope = None;
                self.refresh_owed = true;
                Some(object)
            }
            CatalogOutcome::Failed { error: CatalogError::Unreadable | CatalogError::Stale, .. } => {
                self.refresh_owed = true;
                None
            }
            CatalogOutcome::Failed { .. } | CatalogOutcome::Cancelled { .. } => None,
        }
    }

    /// Note that the object store moved underneath us. The fact is a level; the owed bit is what
    /// turns it into a read.
    pub(crate) fn note_store_moved(&mut self) {
        self.refresh_owed = !self.remount_required;
    }

    /// Note that Recorder committed a ride.
    ///
    /// It arms the same owed bit: a saved ride, a completed removal and a store commit order one
    /// re-read between them, whichever of them a pass sees. The separate entry point names the
    /// producer at the call site.
    pub(crate) fn note_ride_finalized(&mut self) {
        self.refresh_owed = !self.remount_required;
    }
}

// Layout tripwires: an identity, a revision, a count, never a catalog.
const _: () = assert!(core::mem::size_of::<CatalogIntent>() <= 40, "a request with one identity");
const _: () = assert!(core::mem::size_of::<CatalogEffect>() <= 40, "kind fits the existing effect allocation");
const _: () = assert!(core::mem::size_of::<CatalogOutcome>() <= 40, "a token, an identity and a flag");
const _: () = assert!(core::mem::size_of::<CatalogError>() <= 1, "a verdict, not a report");

#[cfg(test)]
impl CatalogState {
    /// Assert the [`new`](CatalogState::new) boot state, field by field. The destructure is
    /// exhaustive, so a field added here must state its boot value too.
    pub(crate) fn assert_boot_state(&self) {
        let CatalogState {
            routes,
            route_ids,
            trips,
            rides,
            ride_profile,
            ride_profile_present,
            ride_profile_for,
            ride_preview,
            ride_preview_for,
            nav_preview,
            nav_preview_route,
            source_revision,
            ride_track_view,
            nav_preview_view,
            detour_preview,
            detour_preview_route,
            ops,
            pending,
            cascade,
            in_flight,
            cleanup_running,
            refresh_owed,
            loaded_scope,
            remount_required,
            read_retry_at,
        } = self;
        assert!(loaded_scope.is_none() && !remount_required && read_retry_at.is_none());
        assert!(routes.is_empty() && route_ids.is_empty(), "no routes catalogued");
        assert!(trips.is_empty(), "no trips catalogued");
        assert!(rides.is_empty(), "no rides catalogued");
        assert_eq!(ride_profile.cols(), Profile::EMPTY.cols(), "the ride-profile buffer is the empty line");
        assert!(!*ride_profile_present && ride_profile_for.is_none(), "no ride profile answered");
        assert!(ride_preview.is_empty() && ride_preview_for.is_none(), "no ride preview cached");
        assert!(nav_preview.is_empty() && nav_preview_route.is_none(), "no route-shape preview cached");
        assert!(
            [*source_revision, *ride_track_view, *nav_preview_view].iter().all(|r| *r == Revision::ZERO),
            "the derived key revisions start at zero — nothing committed, nothing invalidated"
        );
        assert!(detour_preview.is_empty() && detour_preview_route.is_none(), "no detour preview cached");
        assert!(
            pending.is_none() && cascade.is_none() && !*cleanup_running && !*in_flight,
            "no catalog operation admitted or in flight"
        );
        assert!(!*refresh_owed, "nothing has moved the store yet, so no re-read is owed");
        assert_eq!(format!("{ops:?}"), "TokenSource(0)", "no catalog operation has been issued");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_and_whole_route_shapes_have_separate_keys() {
        let mut catalogs = CatalogState::new();
        catalogs.route_ids.push(10).unwrap();
        let whole = catalogs.nav_preview_key(Some(0), false).unwrap();
        let assistant = catalogs.nav_preview_key(Some(0), true).unwrap();
        assert_ne!(whole, assistant);
        assert!(catalogs.accept_nav_preview(Some(assistant), DerivedInput::filled(assistant), &[(1, 2)]));
        assert_eq!(catalogs.nav_preview_for(Some(assistant)), &[(1, 2)]);
        assert!(catalogs.nav_preview_for(Some(whole)).is_empty());
        assert!(!catalogs.nav_preview_answered(whole));
        assert!(!catalogs.accept_nav_preview(Some(whole), DerivedInput::filled(assistant), &[(3, 4)]));
        assert!(catalogs.accept_nav_preview(Some(whole), DerivedInput::filled(whole), &[(5, 6)]));
        assert!(catalogs.nav_preview_for(Some(assistant)).is_empty());
        assert_eq!(core::mem::size_of::<Option<NavPreviewKey>>(), 32);
    }

    #[test]
    fn cleanup_waits_for_an_unrelated_read_and_stops_on_its_own_failure() {
        let mut catalogs = CatalogState::new();
        catalogs.note_store_moved();
        let read = catalogs.next_effect().unwrap();
        catalogs
            .admit_intent(CatalogIntent::CleanupRoutes {
                before_utc: 100,
                store: crate::device_core::StoreIdentity::new(1),
            })
            .unwrap();
        catalogs.apply_outcome(CatalogOutcome::CatalogRead { token: read.token(), scope: None });
        let cleanup = catalogs.next_effect().expect("queued cleanup survives the read");
        assert!(matches!(cleanup, CatalogEffect::CleanupRoute { .. }));
        catalogs.apply_outcome(CatalogOutcome::ObjectRemoved { token: cleanup.token(), object: 1, existed: true });
        let second = catalogs.next_effect().unwrap();
        catalogs.apply_outcome(CatalogOutcome::Failed { token: second.token(), error: CatalogError::RemoveFailed });
        let refresh = catalogs.next_effect().expect("earlier deletion still requires a refresh");
        assert!(matches!(refresh, CatalogEffect::ReadCatalog { .. }));
        catalogs.apply_outcome(CatalogOutcome::CatalogRead { token: refresh.token(), scope: None });
        assert!(catalogs.next_effect().is_none(), "no automatic retry after a failure");
    }

    fn summary() -> RouteSummary {
        RouteSummary {
            name: Default::default(),
            distance_km: 1,
            climb_m: 1,
            bbox: obc_map_scene::BBox { min_lon: 0, min_lat: 0, max_lon: 1, max_lat: 1 },
            start_lon: 0,
            start_lat: 0,
        }
    }

    /// A catalog holding one trip (`id`, stages `stage_ids`) over the routes `route_ids`.
    fn with_trip(id: CatalogObjectId, stage_ids: &[CatalogObjectId], route_ids: &[CatalogObjectId]) -> CatalogState {
        let mut catalogs = CatalogState::new();
        let summaries: heapless::Vec<RouteSummary, MAX_ROUTES> = route_ids.iter().map(|_| summary()).collect();
        catalogs.replace_routes(&summaries, route_ids);
        catalogs.set_trips(&[TripInput { id, name: "Alps", stage_ids }]);
        catalogs
    }

    /// Take the whole cascade, answering each step as the executor would, and stop at the re-read
    /// the finished walk orders, which is the walk's own end marker.
    ///
    /// The loop is bounded on purpose: a cursor that stopped advancing is the bug this helper must
    /// report, and an unbounded loop would hang the suite on it instead of failing.
    fn drain_cascade(catalogs: &mut CatalogState) -> heapless::Vec<CatalogObjectId, 8> {
        let mut removed = heapless::Vec::new();
        for _ in 0..=removed.capacity() {
            let Some(effect) = catalogs.next_effect() else { return removed };
            let CatalogEffect::RemoveObject { token, object, .. } = effect else { return removed };
            let _ = removed.push(object);
            catalogs.apply_outcome(CatalogOutcome::ObjectRemoved { token, object, existed: true });
        }
        panic!("the cascade did not reach the folder in {} steps — the cursor is not advancing", removed.capacity())
    }

    /// The cascade is the domain's ordering, made of the same bounded removal every other delete
    /// uses: each member route in stage order, then the folder. Nothing else is admitted while it
    /// walks.
    #[test]
    fn a_trip_cascade_removes_every_member_before_the_folder() {
        let mut catalogs = with_trip(50, &[10, 20, 30], &[10, 20, 30]);
        catalogs.admit_intent(CatalogIntent::DeleteTrip { id: 50 }).unwrap();

        // Mid-walk the slot is busy, and the refused intent goes back to its producer intact.
        let effect = catalogs.next_effect().expect("the first member");
        let later = CatalogIntent::DeleteRoute { id: 99 };
        assert_eq!(catalogs.admit_intent(later).unwrap_err().rejected, later, "handed back, never lost");
        let CatalogEffect::RemoveObject { token, object, .. } = effect else { panic!("a removal") };
        assert_eq!(object, 10, "stage order, first member first");
        catalogs.apply_outcome(CatalogOutcome::ObjectRemoved { token, object, existed: true });

        assert_eq!(drain_cascade(&mut catalogs).as_slice(), &[20, 30, 50], "the rest, then the folder");
        catalogs.admit_intent(later).expect("the cascade released the slot");
    }

    /// A trip and a route may carry the same id, because a host can number its families from
    /// separate counters, and an id-only cascade would then take the route's file for the folder.
    /// The members are resolved through the trip's own stage list instead.
    #[test]
    fn a_route_that_shares_the_trips_id_is_not_cascaded() {
        // Trip 1 has one member, route 7. Route 1 exists too, and shares the trip's number.
        let mut catalogs = with_trip(1, &[7], &[7, 1]);
        catalogs.admit_intent(CatalogIntent::DeleteTrip { id: 1 }).unwrap();
        for (id, expected) in [(7, CatalogObjectKind::Route), (1, CatalogObjectKind::Trip)] {
            let CatalogEffect::RemoveObject { token, object, kind } = catalogs.next_effect().unwrap() else {
                panic!("expected cascade removal")
            };
            assert_eq!((object, kind), (id, expected));
            catalogs.apply_outcome(CatalogOutcome::ObjectRemoved { token, object, existed: true });
        }
        assert!(!matches!(catalogs.next_effect(), Some(CatalogEffect::RemoveObject { .. })));
    }

    #[test]
    fn an_unrelated_failure_keeps_the_queued_trip_intent() {
        for read in [false, true] {
            let mut catalogs = with_trip(50, &[10], &[10, 99]);
            if read {
                catalogs.refresh_owed = true;
            } else {
                catalogs.admit_intent(CatalogIntent::DeleteRoute { id: 99 }).unwrap();
            }
            let effect = catalogs.next_effect().unwrap();
            catalogs.admit_intent(CatalogIntent::DeleteTrip { id: 50 }).unwrap();
            catalogs.apply_outcome(CatalogOutcome::Failed { token: effect.token(), error: CatalogError::Unreadable });
            assert_eq!(drain_cascade(&mut catalogs).as_slice(), &[10, 50]);
        }
    }

    /// Failure preserves the trip and unfinished members; an explicit retry can pass earlier absence.
    #[test]
    fn a_failed_member_stops_the_cascade_until_explicit_retry() {
        let mut catalogs = with_trip(50, &[10, 20], &[10, 20]);
        catalogs.admit_intent(CatalogIntent::DeleteTrip { id: 50 }).unwrap();
        let first = catalogs.next_effect().unwrap();
        catalogs.apply_outcome(CatalogOutcome::ObjectRemoved { token: first.token(), object: 10, existed: true });
        let second = catalogs.next_effect().unwrap();
        catalogs.apply_outcome(CatalogOutcome::Failed { token: second.token(), error: CatalogError::RemoveFailed });
        let read = catalogs.next_effect().unwrap();
        assert!(matches!(read, CatalogEffect::ReadCatalog { .. }), "earlier success still needs a reload");
        catalogs.apply_outcome(CatalogOutcome::CatalogRead { token: read.token(), scope: None });
        assert!(catalogs.next_effect().is_none(), "no automatic continuation or retry");
        catalogs.admit_intent(CatalogIntent::DeleteTrip { id: 50 }).unwrap();
        let first = catalogs.next_effect().unwrap();
        assert!(matches!(first, CatalogEffect::RemoveObject { object: 10, kind: CatalogObjectKind::Route, .. }));
        catalogs.apply_outcome(CatalogOutcome::ObjectRemoved { token: first.token(), object: 10, existed: false });
        assert_eq!(drain_cascade(&mut catalogs).as_slice(), &[20, 50]);
    }

    /// The walk holds a cursor, not a copy of the member list, so ordinal `n` keeps naming the same
    /// member only while nothing rewrites the list under it. Nothing does: a host re-feed reads the
    /// trip's own stage refs verbatim, and the cascade leaves the folder alone until its last step,
    /// so a member already removed is still named as a dangling id.
    #[test]
    fn a_catalog_re_feed_mid_cascade_does_not_move_the_cursor() {
        let mut catalogs = with_trip(50, &[10, 20, 30], &[10, 20, 30]);
        catalogs.admit_intent(CatalogIntent::DeleteTrip { id: 50 }).unwrap();

        let effect = catalogs.next_effect().expect("the first member");
        let CatalogEffect::RemoveObject { token, object, .. } = effect else { panic!("a removal") };
        assert_eq!(object, 10);
        catalogs.apply_outcome(CatalogOutcome::ObjectRemoved { token, object, existed: true });

        // The host re-reads the store between two steps: route 10 is gone from the route catalog,
        // and the trip is re-fed from the untouched `.obt` — dangling stage ref and all.
        let summaries: heapless::Vec<RouteSummary, MAX_ROUTES> = (0..2).map(|_| summary()).collect();
        catalogs.replace_routes(&summaries, &[20, 30]);
        catalogs.set_trips(&[TripInput { id: 50, name: "Alps", stage_ids: &[10, 20, 30] }]);

        assert_eq!(drain_cascade(&mut catalogs).as_slice(), &[20, 30, 50], "the walk resumes where it was");
    }

    /// A trip that vanished from the resident catalog mid-walk ends the walk at the folder rather
    /// than re-reading a member list that is no longer there.
    #[test]
    fn a_cascade_over_a_vanished_trip_still_removes_the_folder() {
        let mut catalogs = CatalogState::new();
        catalogs.admit_intent(CatalogIntent::DeleteTrip { id: 50 }).unwrap();
        assert_eq!(drain_cascade(&mut catalogs).as_slice(), &[50], "no members to walk, the folder still goes");
    }

    /// A completed removal orders exactly one re-read: the store moved, so the resident catalogs
    /// are behind it, and taking the owed bit is what spends it.
    ///
    /// Both `existed` verdicts arm it. An object the store did not have may still be a resident
    /// row, and the only way to find out is to read.
    #[test]
    fn a_completed_removal_orders_exactly_one_re_read() {
        for existed in [true, false] {
            let mut catalogs = CatalogState::new();
            catalogs.admit_intent(CatalogIntent::DeleteRoute { id: 10 }).unwrap();
            let removal = catalogs.next_effect().expect("the removal");
            assert!(catalogs.next_effect().is_none(), "nothing else while the removal is out");

            catalogs.apply_outcome(CatalogOutcome::ObjectRemoved { token: removal.token(), object: 10, existed });
            let read = catalogs.next_effect().expect("the completed removal orders a re-read");
            assert!(
                matches!(read, CatalogEffect::ReadCatalog { .. }),
                "and it is a read: {read:?} (existed {existed})"
            );

            catalogs.apply_outcome(CatalogOutcome::CatalogRead { token: read.token(), scope: None });
            assert!(catalogs.next_effect().is_none(), "and one — a second would walk the whole store again");
        }
    }

    /// The cascade orders one re-read, after the folder, never one per member. The cursor is `Some`
    /// for every member step and `None` by the time the folder's answer arrives.
    #[test]
    fn a_cascade_orders_one_re_read_after_the_folder() {
        let mut catalogs = with_trip(50, &[10, 20, 30], &[10, 20, 30]);
        catalogs.admit_intent(CatalogIntent::DeleteTrip { id: 50 }).unwrap();

        // `None` is the re-read; every other step names the object it removed.
        let mut steps: heapless::Vec<Option<CatalogObjectId>, 8> = heapless::Vec::new();
        for _ in 0..=steps.capacity() {
            let Some(effect) = catalogs.next_effect() else { break };
            match effect {
                CatalogEffect::CleanupRoute { .. } | CatalogEffect::RemoveReview { .. } => panic!("unexpected cleanup"),
                CatalogEffect::RemoveObject { token, object, .. } => {
                    let _ = steps.push(Some(object));
                    catalogs.apply_outcome(CatalogOutcome::ObjectRemoved { token, object, existed: true });
                }
                CatalogEffect::ReadCatalog { token } => {
                    let _ = steps.push(None);
                    catalogs.apply_outcome(CatalogOutcome::CatalogRead { token, scope: None });
                }
            }
        }
        assert_eq!(
            steps.as_slice(),
            &[Some(10), Some(20), Some(30), Some(50), None],
            "every member, then the folder, then exactly one re-read"
        );
    }

    #[test]
    fn a_removal_the_store_refused_orders_no_re_read() {
        let mut catalogs = CatalogState::new();
        catalogs.admit_intent(CatalogIntent::DeleteRoute { id: 10 }).unwrap();
        let effect = catalogs.next_effect().expect("the removal");
        catalogs.apply_outcome(CatalogOutcome::Failed { token: effect.token(), error: CatalogError::RemoveFailed });
        assert!(catalogs.next_effect().is_none(), "a refused removal moved nothing, so it orders nothing");
    }

    /// The placement path must land the same state the by-value path builds.
    #[test]
    fn init_in_place_matches_new() {
        CatalogState::new().assert_boot_state();

        let mut slot = core::mem::MaybeUninit::<CatalogState>::uninit();
        // SAFETY: `slot` is a valid, aligned, exclusively-owned region for one `CatalogState`.
        let placed = unsafe {
            CatalogState::init_in_place(slot.as_mut_ptr());
            slot.assume_init_ref()
        };
        placed.assert_boot_state();
    }
}
