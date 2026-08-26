//! [`CatalogState`] — the resident route / ride / trip catalogs, keyed by durable object ids.
//!
//! One component owns every id ↔ summary pairing and every piece of state keyed by catalog
//! *identity* (the #450 contract): the route and ride catalogs with their durable ids, the trip
//! folders resolving stage ids against the route catalog, and the executor-filled derived targets
//! (the viewed ride's profile/preview, the route overview's shape preview) held under the
//! [`derived`](crate::device_core::derived) keys of #1437 — durable identity plus a source and view
//! revision, so no cache key has to be walked across a live rescan.
//!
//! **The pairing invariant is encoded here, not policed by callers**: ids and summaries are only
//! ever replaced together ([`replace_routes`](CatalogState::replace_routes) /
//! [`replace_rides`](CatalogState::replace_rides)), read out together as a [`RouteEntry`] /
//! [`RideEntry`], and remapped together. Storage keeps the summaries contiguous (the `&[Summary]`
//! slice every screen renders from) with the id column alongside — fusing them into one
//! `Vec<{id, summary}>` would add 2 B + repr padding per slot (~150 B resident on the board), which
//! the epic's no-RAM-growth rule (#792 rule 2) outranks; the entry types keep the invariant
//! structural at every read/write seam instead.
//!
//! `App` remains the composition root: screen-stack remaps and `Activity` key remaps stay there
//! (this component never sees a `Screen`), driven by the old-id snapshot each `replace_*` returns.

use obc_route::Profile;

use crate::app::NAV_PREVIEW_MAX;
use crate::device_core::derived::{DerivedInput, NavPreviewKey, RideTrackKey};
use crate::device_core::Revision;
use crate::placement::define_placement_constructors;
use crate::retention::{RideRetentionRecord, RouteRetentionMeta};
use crate::ride::{RideCatalog, RideSummary, MAX_RIDES, UI_RIDES_CAP};
use crate::route::{Catalog, RouteSummary, MAX_ROUTES};
use crate::trip::{TripInput, TripSummary, Trips, MAX_TRIPS};
use crate::CatalogObjectId;

/// One route-catalog entry: the durable object id and its summary, handed out **together** so the
/// id ↔ summary pairing is a type, not a convention (issue #802's catalog invariant).
#[derive(Debug, Clone, Copy)]
pub struct RouteEntry<'a> {
    /// The route's durable object id (#450) — what survives a live rescan.
    pub id: CatalogObjectId,
    /// The resident summary the menus render.
    pub summary: &'a RouteSummary,
}

/// One ride-catalog entry — the ride-namespace twin of [`RouteEntry`].
#[derive(Debug, Clone, Copy)]
pub struct RideEntry<'a> {
    /// The ride's durable object id.
    pub id: CatalogObjectId,
    /// The resident summary the Rides screen renders.
    pub summary: &'a RideSummary,
}

/// The snapshot of a catalog's ids **before** a replacement — what
/// [`CatalogState::remap_route`] / [`CatalogState::remap_ride`] resolve an old index through to
/// find its new home. Returned by the `replace_*` methods so `App` can re-point the screen stack
/// and `Activity` keys with the exact mapping the component itself used.
pub(crate) type OldRouteIds = heapless::Vec<CatalogObjectId, MAX_ROUTES>;
/// The ride twin of [`OldRouteIds`].
pub(crate) type OldRideIds = heapless::Vec<CatalogObjectId, UI_RIDES_CAP>;

/// The resident catalogs + identity-keyed view caches. See the module docs.
pub(crate) struct CatalogState {
    /// The resident route catalog (summaries) — what the Route menu lists;
    /// `Activity::active_route` indexes it.
    routes: Catalog,
    /// Each route's **durable object id**, pairwise with [`routes`](CatalogState::routes) (#450) —
    /// only ever written in lock step with it (the component's whole point).
    route_ids: heapless::Vec<CatalogObjectId, MAX_ROUTES>,
    /// Each route's device-local **retention meta** (level + `last_used`), pairwise with
    /// [`route_ids`](CatalogState::route_ids) (epic #638, S3). Carried alongside the catalog — never
    /// in the byte-pinned OBCR file — and **remapped by identity** across a rescan (a surviving
    /// route keeps its meta, a new id defaults to [`Never`](crate::Retention::Never)), so the sweep
    /// always reads the meta paired with the route it belongs to. The host re-pushes fresh sidecar
    /// values through [`set_route_meta`](CatalogState::set_route_meta) after each scan.
    route_meta: heapless::Vec<RouteRetentionMeta, MAX_ROUTES>,
    /// The resident **trip** catalog (epic #526): grouped-route folders resolving their stage
    /// route ids against [`route_ids`](CatalogState::route_ids); re-resolved on every route
    /// replacement so an appeared/vanished route re-files.
    trips: Trips,
    /// The resident ride catalog (summaries) — what the Rides screen lists (epic #447, P7).
    rides: RideCatalog,
    /// Each ride's durable object id, pairwise with [`rides`](CatalogState::rides).
    ride_ids: heapless::Vec<CatalogObjectId, UI_RIDES_CAP>,
    /// The **full** compact ride-retention inventory (finding #876-2): every stored ride's
    /// `id + synced + synced_at`, up to [`MAX_RIDES`], independent of the newest-[`UI_RIDES_CAP`]
    /// display catalog above. The retention sweep + eager `synced_at` stamp read this — so an older
    /// synced+expired ride the menu never shows is still reachable by expiry. Fed by the host
    /// ([`set_ride_retention_inventory`](CatalogState::set_ride_retention_inventory)); a plain
    /// [`replace_rides`](CatalogState::replace_rides) also seeds it from the visible summaries so a
    /// host that never streams the full view still expires the rides it does surface.
    ride_inventory: heapless::Vec<RideRetentionRecord, MAX_RIDES>,
    /// The **viewed ride's** recorded-track elevation profile (epic #678 T2 / #680) — the Ride
    /// detail's band source, host-filled once per detail entry. `None` while unanswered.
    ride_profile: Profile,
    /// Whether [`ride_profile`](CatalogState::ride_profile) contains a successful host answer.
    /// Kept separate so the board can stream directly into the resident buffer without returning
    /// a ~5 KiB value through its task frame.
    ride_profile_present: bool,
    /// The [`RideTrackKey`] [`ride_profile`](CatalogState::ride_profile) was **answered** for — a
    /// failed fill parks the same key with `present == false`, so a dead file is answered once
    /// rather than re-streamed every pass.
    ride_profile_for: Option<RideTrackKey>,
    /// The viewed ride's decimated recorded-track shape polyline (#678 rework 3), host-filled in
    /// the same drain as the profile.
    ride_preview: heapless::Vec<(i32, i32), NAV_PREVIEW_MAX>,
    /// The key the [`ride_preview`](CatalogState::ride_preview) was handed in for.
    ride_preview_for: Option<RideTrackKey>,
    /// The Route overview's decimated route-shape preview polyline (#685 §4), host-decimated and
    /// handed in via [`accept_nav_preview`](CatalogState::accept_nav_preview).
    nav_preview: heapless::Vec<(i32, i32), NAV_PREVIEW_MAX>,
    /// The [`NavPreviewKey`] the [`nav_preview`](CatalogState::nav_preview) was handed in for — the
    /// staleness key (the render gates on it so an old plan's shape can never draw under a
    /// different route, and a *re-committed* one can never draw under fresh geometry).
    nav_preview_route: Option<NavPreviewKey>,
    /// The revision of the object bytes this component knows about: bumped by
    /// [`note_commit`](CatalogState::note_commit) whenever something committed new bytes over a
    /// durable identity. It is the `source` half of every derived key, and the reason a route
    /// upload that *replaces* a stored route cannot leave its old preview standing.
    ///
    /// Deliberately one store-wide counter rather than one per namespace: a re-commit is rare, the
    /// only cost of the coarser key is one extra derived read when an unrelated route is replaced
    /// while a ride detail is open, and a rides-only revision would have nothing to bump it — a
    /// finalised ride's bytes never change.
    source_revision: Revision,
    /// The ride-track view generation — bumped by an explicit invalidate (an in-place fill that
    /// starts), so an abandoned fill leaves the need up instead of a half-written buffer answered.
    ride_track_view: Revision,
    /// The nav-preview view generation — bumped by
    /// [`invalidate_nav_preview`](CatalogState::invalidate_nav_preview) so every committed plan
    /// starts preview-less even when the route identity and its bytes are unchanged.
    nav_preview_view: Revision,
    /// The **detour preview** polyline (#882): the planned-but-uncommitted detour's decimated
    /// shape, drawn by the Detour preview screen *over* the still-active original route.
    /// Host-filled when the detour plan completes; cleared on commit, cancel, or route change.
    detour_preview: heapless::Vec<(i32, i32), NAV_PREVIEW_MAX>,
    /// The route index the [`detour_preview`](CatalogState::detour_preview) was planned against —
    /// its staleness key (a route swap or rescan mid-preview blanks the overlay rather than
    /// drawing a stale detour over different geometry).
    detour_preview_route: Option<usize>,
    /// The token source for the catalog's one operation (#1438). One operation is in flight at a
    /// time — the domain's single effect slot — so one source is all it needs.
    ops: crate::device_core::TokenSource<crate::device_core::CatalogTag>,
    /// An admitted [`CatalogIntent`] that has not become an effect yet. Capacity one: later work
    /// stays with whoever asked for it, where it can still be superseded or cancelled.
    ///
    /// A [`DeleteTrip`](CatalogIntent::DeleteTrip) stays here for the **whole** cascade rather than
    /// one effect — see [`cascade`](CatalogState::cascade) — so the same one slot that delays a
    /// second delete also delays one behind a running cascade.
    pending: Option<CatalogIntent>,
    /// How far the trip cascade has walked: the **stage ordinal** the next member removal takes,
    /// or `None` when no cascade is running. Two bytes, and that is the whole resident cost of the
    /// cascade — the member ids are already resident in
    /// [`trips`](CatalogState::trips)`[..].stage_ids`, kept verbatim exactly so a delete has
    /// something to key on, so there is no member buffer to add (#1491).
    cascade: Option<u8>,
    /// Whether an effect is out with the executor. Its outcome clears this, which is what lets the
    /// next intent go out.
    in_flight: bool,
    /// The resident catalogs are behind the store, and a re-read has not gone out yet. Armed by
    /// [`note_store_moved`](CatalogState::note_store_moved) (the store-revision fact), by a
    /// completed removal, and by a read the store could not answer; spent by
    /// [`next_effect`](CatalogState::next_effect) once nothing is pending.
    ///
    /// A **bit, not a counter**, and that is the whole coalescing rule: a delete that also moves
    /// the store arms the same bit twice and costs one read, not two.
    refresh_owed: bool,
}

impl CatalogState {
    define_placement_constructors!(
        /// Empty catalogs, nothing cached — the boot state.
        pub(crate) fn new();
        /// Initialize `slot` **in place** to the [`new`](CatalogState::new) state — the placement
        /// path the firmware boots through (the catalogs are several KB; nothing here may form a
        /// by-value `CatalogState` on the stack).
        pub(crate) unsafe fn init_in_place;
        fields {
            routes: Catalog::new(),
            route_ids: heapless::Vec::new(),
            route_meta: heapless::Vec::new(),
            trips: Trips::new(),
            rides: RideCatalog::new(),
            ride_ids: heapless::Vec::new(),
            ride_inventory: heapless::Vec::new(),
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
            refresh_owed: false,
        }
    );

    // ---- route catalog ----

    /// The resident route summaries (what `Ctx`/`Render` hand the screens).
    pub(crate) fn routes(&self) -> &[RouteSummary] {
        &self.routes
    }

    /// Each route's durable id, pairwise with [`routes`](CatalogState::routes).
    pub(crate) fn route_ids(&self) -> &[CatalogObjectId] {
        &self.route_ids
    }

    /// The durable id at catalog index `idx`, or `None` out of range — drain-time id resolution
    /// (#837: a vanished subject resolves to nothing).
    pub(crate) fn route_id_at(&self, idx: usize) -> Option<CatalogObjectId> {
        self.route_ids.get(idx).copied()
    }

    /// The catalog index currently holding durable id `id`, or `None` when it isn't resident.
    pub(crate) fn route_index_of(&self, id: CatalogObjectId) -> Option<usize> {
        self.route_ids.iter().position(|&x| x == id)
    }

    /// How many routes are resident.
    pub(crate) fn route_len(&self) -> usize {
        self.routes.len()
    }

    /// Replace the route catalog from the host's store (`ids` pairwise with `summaries`; entries
    /// past [`MAX_ROUTES`] are ignored), re-resolve the trip folders against the new ids, and
    /// return the old id column so the caller can remap every held index by identity
    /// ([`remap_route`](CatalogState::remap_route)).
    pub(crate) fn replace_routes(&mut self, summaries: &[RouteSummary], ids: &[CatalogObjectId]) -> OldRouteIds {
        let old_ids = self.route_ids.clone();
        let old_meta = self.route_meta.clone();
        self.routes.clear();
        self.route_ids.clear();
        self.route_meta.clear();
        for (s, &id) in summaries.iter().zip(ids).take(MAX_ROUTES) {
            let _ = self.routes.push(s.clone());
            let _ = self.route_ids.push(id);
            // Carry each surviving route's retention meta across the rescan by identity (#638 S3):
            // its id's old slot → its old meta, a genuinely new id → the default (Never). The host
            // re-pushes fresh sidecar values via `set_route_meta` right after, but this keeps the
            // meta coherent for a host that doesn't (tests, the map-only build) — the "kept coherent
            // through the same remap machinery" contract.
            let meta = old_ids.iter().position(|&o| o == id).map(|p| old_meta[p]).unwrap_or_default();
            let _ = self.route_meta.push(meta);
        }
        // Trips resolve stage *ids* into catalog indices, so a catalog replacement re-points them:
        // a route that appeared re-files, one that vanished dangles (dropped from the resolved
        // list, its stats no longer summed). Re-resolved here — before the caller's stack walk —
        // so the Route menu's remap sees the regrouped folders (epic #526).
        for t in self.trips.iter_mut() {
            t.reresolve(&self.routes, &self.route_ids);
        }
        old_ids
    }

    /// Old route index → new route index by durable identity: the id `old_ids` recorded at `idx`,
    /// found in the replaced catalog — or `None` when that route vanished. The one mapping every
    /// held index (`active_route`, cache keys, open screens) follows across a rescan (#450).
    pub(crate) fn remap_route(&self, old_ids: &[CatalogObjectId], idx: usize) -> Option<usize> {
        let id = *old_ids.get(idx)?;
        self.route_index_of(id)
    }

    // ---- route retention (epic #638, S3) ----

    /// Each route's retention meta, pairwise with [`route_ids`](CatalogState::route_ids) — the
    /// sweep's per-route input column.
    pub(crate) fn route_metas(&self) -> &[RouteRetentionMeta] {
        &self.route_meta
    }

    /// Push the host's fresh per-route retention metas (from the SD sidecar), pairwise with the
    /// **current** [`route_ids`](CatalogState::route_ids) — the host calls this right after
    /// [`replace_routes`](CatalogState::replace_routes) so the app mirrors device-durable retention.
    /// Excess metas (a host that fed more than the catalog holds) are ignored; a short slice leaves
    /// the remaining routes at their remap-carried value.
    pub(crate) fn set_route_meta(&mut self, metas: &[RouteRetentionMeta]) {
        for (slot, &m) in self.route_meta.iter_mut().zip(metas) {
            *slot = m;
        }
    }

    /// Optimistically stamp route `id`'s `last_used` in the resident meta (the sweep/activation
    /// mirror of the host's sidecar write) so a re-derivation before the host's rescan lands doesn't
    /// re-enqueue the same stamp. A no-op if the id isn't resident.
    pub(crate) fn stamp_route_last_used(&mut self, id: CatalogObjectId, utc: u32) {
        if let Some(p) = self.route_ids.iter().position(|&x| x == id) {
            self.route_meta[p].last_used_utc = utc;
        }
    }

    // ---- trips ----

    /// Replace the resident trip catalog (epic #526, TR2), resolving each trip's stage ids against
    /// the current route catalog. Trips past [`MAX_TRIPS`] are ignored (the host warns and lists
    /// the first N). Call **after** the routes are set so the stage ids resolve; a later
    /// [`replace_routes`](CatalogState::replace_routes) re-resolves them in place.
    pub(crate) fn set_trips(&mut self, trips: &[TripInput]) {
        self.trips.clear();
        for input in trips.iter().take(MAX_TRIPS) {
            let _ = self.trips.push(TripSummary::resolve(input, &self.routes, &self.route_ids));
        }
    }

    /// The resident trip catalog — the grouped-route folders.
    pub(crate) fn trips(&self) -> &[TripSummary] {
        &self.trips
    }

    /// Whether the route at catalog index `idx` is **filed** into some trip (epic #526) — a filed
    /// route shows only inside its folder.
    pub(crate) fn route_filed(&self, idx: usize) -> bool {
        let i = idx as u16;
        self.trips.iter().any(|t| t.stage_indices.contains(&i))
    }

    // ---- ride catalog ----

    /// The resident ride summaries — what the Rides screen lists.
    pub(crate) fn rides(&self) -> &[RideSummary] {
        &self.rides
    }

    /// Each ride's durable id, pairwise with [`rides`](CatalogState::rides).
    pub(crate) fn ride_ids(&self) -> &[CatalogObjectId] {
        &self.ride_ids
    }

    /// The full compact ride-retention inventory the sweep reads (finding #876-2) — every stored
    /// ride's `id + synced + synced_at`, not just the newest-[`UI_RIDES_CAP`] the menu shows.
    pub(crate) fn ride_records(&self) -> &[RideRetentionRecord] {
        &self.ride_inventory
    }

    /// Overwrite the compact ride-retention inventory from the host's full store scan (finding
    /// #876-2). Independent of [`replace_rides`](CatalogState::replace_rides): the host streams
    /// **every** stored ride (up to [`MAX_RIDES`]) here so retention sees rides beyond the display
    /// catalog; entries past the cap are ignored.
    pub(crate) fn set_ride_retention_inventory(&mut self, records: &[RideRetentionRecord]) {
        self.ride_inventory.clear();
        for r in records.iter().take(MAX_RIDES) {
            let _ = self.ride_inventory.push(*r);
        }
    }

    /// Optimistically stamp ride `id`'s `synced_at` in the **inventory** (the sweep's mirror of the
    /// host's sidecar write, the full-inventory twin of
    /// [`stamp_ride_synced_at`](CatalogState::stamp_ride_synced_at)) so a re-derivation before the
    /// host's rescan lands doesn't re-enqueue the same stamp for a ride outside the display catalog.
    /// Only ever fills a `0` stamp. A no-op if the id isn't in the inventory.
    pub(crate) fn stamp_inventory_synced_at(&mut self, id: CatalogObjectId, utc: u32) {
        if let Some(r) = self.ride_inventory.iter_mut().find(|r| r.id == id && r.synced_at_utc == 0) {
            r.synced_at_utc = utc;
        }
    }

    /// The paired `{id, summary}` at ride-catalog index `idx` — the ride twin of
    /// [`route_entry`](CatalogState::route_entry).
    pub(crate) fn ride_entry(&self, idx: usize) -> Option<RideEntry<'_>> {
        Some(RideEntry { id: *self.ride_ids.get(idx)?, summary: self.rides.get(idx)? })
    }

    /// How many rides are resident.
    pub(crate) fn ride_len(&self) -> usize {
        self.rides.len()
    }

    /// Replace the ride catalog (`ids` pairwise with `summaries`; entries past [`UI_RIDES_CAP`]
    /// ignored) and remap this component's own identity-keyed view caches — the answered-profile
    /// and preview keys move with the ride they were filled for, and drop (buffer cleared) when it
    /// vanished. Returns the old id column for the caller's screen/`Activity` remap.
    pub(crate) fn replace_rides(&mut self, summaries: &[RideSummary], ids: &[CatalogObjectId]) -> OldRideIds {
        let old_ids = self.ride_ids.clone();
        self.rides.clear();
        self.ride_ids.clear();
        // Seed the compact retention inventory from the visible summaries as a fallback (finding
        // #876-2): a host that never streams the full store view still expires the rides it does
        // surface. A retention-aware host (the board) overwrites this with the full up-to-MAX_RIDES
        // view via `set_ride_retention_inventory` right after, so the sweep sees rides beyond the
        // newest UI_RIDES_CAP too.
        self.ride_inventory.clear();
        for (s, &id) in summaries.iter().zip(ids).take(UI_RIDES_CAP) {
            let _ = self.rides.push(s.clone());
            let _ = self.ride_ids.push(id);
            let _ =
                self.ride_inventory.push(RideRetentionRecord { id, synced: s.synced, synced_at_utc: s.synced_at_utc });
        }
        // The view caches need no remap at all: their keys name a *durable ride identity*, so a
        // surviving ride keeps its answer (no re-stream) and a vanished one simply stops matching
        // any key the need can produce. That is the whole point of #1437's keyed derived data —
        // the index walk that used to live here was the bug surface it removes.
        old_ids
    }

    /// Old ride index → new ride index by durable identity — the ride twin of
    /// [`remap_route`](CatalogState::remap_route).
    pub(crate) fn remap_ride(&self, old_ids: &[CatalogObjectId], idx: usize) -> Option<usize> {
        let id = *old_ids.get(idx)?;
        self.ride_ids.iter().position(|&x| x == id)
    }

    /// Optimistically stamp ride `id`'s `synced_at` in the resident summary (the sweep's mirror of
    /// the host's sidecar write) so a re-derivation before the host's rescan lands doesn't re-enqueue
    /// the same stamp. Only ever fills a `0` stamp (never re-stamps). A no-op if the id isn't
    /// resident. Returns whether it changed a summary (drives the map repaint).
    pub(crate) fn stamp_ride_synced_at(&mut self, id: CatalogObjectId, utc: u32) -> bool {
        if let Some(p) = self.ride_ids.iter().position(|&x| x == id) {
            if self.rides[p].synced_at_utc == 0 {
                self.rides[p].synced_at_utc = utc;
                return true;
            }
        }
        false
    }

    // ---- keyed derived data (#1437) ----
    //
    // Two derived reads, two [`DerivedNeeds`](crate::device_core::derived::DerivedNeeds) slots, and
    // one rule for both: the answer is stored under the key the need carried, and every read
    // compares that key with the key the need would carry *now*. Nothing is remapped, nothing is
    // "invalidated on rescan" — a subject change, fresh bytes, or an explicit invalidate simply
    // produces a different key, and the old answer becomes unreachable in the same instant.

    /// The derived **ride-track** key for the ride at catalog index `viewed_ride`, or `None` when
    /// no detail is open (or its subject vanished) — the key the need carries and the key an answer
    /// must bring back.
    pub(crate) fn ride_track_key(&self, viewed_ride: Option<usize>) -> Option<RideTrackKey> {
        let ride = *self.ride_ids.get(viewed_ride?)?;
        Some(RideTrackKey { ride, source: self.source_revision, view: self.ride_track_view })
    }

    /// The derived **nav-preview** key for the route at catalog index `active_route` — the route
    /// twin of [`ride_track_key`](Self::ride_track_key).
    pub(crate) fn nav_preview_key(&self, active_route: Option<usize>) -> Option<NavPreviewKey> {
        let route = *self.route_ids.get(active_route?)?;
        Some(NavPreviewKey { route, source: self.source_revision, view: self.nav_preview_view })
    }

    /// Whether the ride-track need for `key` is already answered — a recorded failure counts, so a
    /// dead file is read once rather than on every pass.
    ///
    /// The **profile** is the authoritative answer, not the profile *and* the preview, and that is
    /// deliberate: it is the rule the index-keyed `ride_profile_answered_for` had before #1437, and
    /// all three hosts fill both targets in one drain of the same read (the board's `flat_store`
    /// pair, the simulator, and `obc-host-core`'s dispatcher). Requiring both would make a host that
    /// legitimately has no track shape to hand in re-fire the read on every pass forever — the exact
    /// grind the level is built to avoid.
    pub(crate) fn ride_track_answered(&self, key: RideTrackKey) -> bool {
        self.ride_profile_for == Some(key)
    }

    /// Whether the nav-preview need for `key` is already answered.
    pub(crate) fn nav_preview_answered(&self, key: NavPreviewKey) -> bool {
        self.nav_preview_route == Some(key)
    }

    /// Note that something committed **new bytes over a durable identity** (an upload that replaced
    /// a stored object, a spliced route). Every derived key moves with it, so an answer produced
    /// from the previous bytes stops matching — the one case identity alone cannot catch.
    pub(crate) fn note_commit(&mut self) {
        self.source_revision = self.source_revision.next();
    }

    /// Accept a keyed ride-**profile** answer. A stale key changes nothing at all: the payload is
    /// dropped and the need stays up, so a fill that finished after the rider moved on cannot land
    /// on the ride they are looking at now. Returns whether it was accepted.
    ///
    /// That refusal only *bites* once an executor carries the key it was asked with. The temporary
    /// The keyed ride-track answer derives `current` from the
    /// live subject and hands the same value as `input.key`, so it can never refuse — the legacy
    /// command it answers carries no key back. DC6 #1439 is where the guard starts holding; until
    /// then the late-answer misattribution stays the characterized defect the DC1 traces record.
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

    /// Borrow the one resident profile buffer for an in-place fill, **invalidating** the ride-track
    /// view first: until the matching accept lands, the need carries a new key and re-emits. An
    /// abandoned fill therefore leaves a need up rather than a half-written buffer marked answered.
    pub(crate) fn begin_ride_profile_fill(&mut self) -> &mut Profile {
        self.ride_profile_present = false;
        self.ride_track_view = self.ride_track_view.next();
        &mut self.ride_profile
    }

    /// Accept a keyed ride-**preview** answer (≤ [`NAV_PREVIEW_MAX`] points, more truncated), under
    /// the same staleness rule as the profile.
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

    /// The resident ride profile **iff** it was answered for `key` — the buffer is only reachable
    /// through the exact key it was filled for.
    pub(crate) fn ride_profile_for(&self, key: Option<RideTrackKey>) -> Option<&Profile> {
        (self.ride_profile_present && key.is_some() && self.ride_profile_for == key).then_some(&self.ride_profile)
    }

    /// The ride-shape preview for `key`, or the empty slice when missing or stale — the screens
    /// draw whatever this hands them, so a stale shape is unreachable.
    pub(crate) fn ride_preview_for(&self, key: Option<RideTrackKey>) -> &[(i32, i32)] {
        if key.is_some() && self.ride_preview_for == key {
            &self.ride_preview
        } else {
            &[]
        }
    }

    /// Release the ride profile + preview once they stop matching the live key (#680): the detail
    /// exited or moved subjects. The key gate already makes them unreachable; dropping the keys is
    /// what lets the need re-fire when the rider comes back.
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

    /// Accept a keyed **nav-preview** answer (#685 §4) — the previewed route's decimated shape.
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

    /// Invalidate the nav preview: drop it **and bump the view generation**, so every committed
    /// plan starts preview-less (#685 §4) even when the route identity and its bytes are unchanged.
    /// The bump is what makes this an invalidate rather than a clear a late answer could undo.
    pub(crate) fn invalidate_nav_preview(&mut self) {
        self.nav_preview.clear();
        self.nav_preview_route = None;
        self.nav_preview_view = self.nav_preview_view.next();
    }

    /// Hand in a planned detour's decimated polyline (#882), keyed to the route it was planned
    /// against ([`detour_preview_for`](CatalogState::detour_preview_for) gates on the same key).
    pub(crate) fn set_detour_preview(&mut self, pts: &[(i32, i32)], active_route: Option<usize>) {
        self.detour_preview.clear();
        for &p in pts.iter().take(NAV_PREVIEW_MAX) {
            let _ = self.detour_preview.push(p);
        }
        self.detour_preview_route = active_route;
    }

    /// The detour-preview polyline for `active_route`, or the empty slice when missing/stale.
    pub(crate) fn detour_preview_for(&self, active_route: Option<usize>) -> &[(i32, i32)] {
        if self.detour_preview_route.is_some() && self.detour_preview_route == active_route {
            &self.detour_preview
        } else {
            &[]
        }
    }

    /// Clear the detour preview and its key — a commit, cancel, or failure ends the preview.
    pub(crate) fn clear_detour_preview(&mut self) {
        self.detour_preview.clear();
        self.detour_preview_route = None;
    }
}

// ==================== the Catalog domain protocol (#1436) ====================
//
// CatalogMachine owns every ordering the host used to improvise: delete-then-refresh, the trip
// cascade's member-then-folder order, and the identity remap a refresh implies. The store executor
// is left with two operations — read the catalog, remove an object — and no say in what either of
// them means.
//
// **Every re-read is ordered here.** Three events say the resident catalogs are behind the store —
// the store moved underneath us, a removal completed, a read failed — and all three arm one bit
// (#1541). No executor decides whether, when, or how many times a refresh happens.
//
// Bulk stays out. A catalog read fills the resident catalogs through their existing feeders and the
// outcome reports only that the operation is over.

use crate::device_core::{CatalogTag, OperationToken};

/// What the UI (or another domain) asks of the catalog.
///
/// Expiry arrives here too: `RetentionMachine` advances first and sends its deletions as these same
/// intents, so an auto-expired ride leaves through exactly the path a rider-deleted one does.
///
/// Every variant is a **deletion**. A re-read is not asked for from outside: it is owed by the
/// domain itself ([`note_store_moved`](CatalogState::note_store_moved) and the two arms of
/// [`apply_outcome`](CatalogState::apply_outcome)), and lives in one bit rather than in this slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogIntent {
    /// Delete one route.
    DeleteRoute { id: CatalogObjectId },
    /// Delete one ride.
    DeleteRide { id: CatalogObjectId },
    /// Delete one trip **and its member routes** — the cascade, whose order the domain owns.
    DeleteTrip { id: CatalogObjectId },
}

/// One bounded physical catalog operation, carrying the [`OperationToken`] the domain issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogEffect {
    /// Re-read the object store into the resident catalogs.
    ReadCatalog { token: OperationToken<CatalogTag> },
    /// Remove one object. Deliberately namespace-free: routes, rides and trips are all objects to
    /// the store, and it is the domain that knows which cascade step this is.
    RemoveObject { token: OperationToken<CatalogTag>, object: CatalogObjectId },
}

impl CatalogEffect {
    /// The operation this effect belongs to.
    pub fn token(&self) -> OperationToken<CatalogTag> {
        match self {
            CatalogEffect::ReadCatalog { token } | CatalogEffect::RemoveObject { token, .. } => *token,
        }
    }
}

/// Why a catalog operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogError {
    /// The store could not be read.
    Unreadable,
    /// The store refused or failed the removal. A *missing* object is not this — see
    /// [`ObjectRemoved`](CatalogOutcome::ObjectRemoved)'s `existed`.
    RemoveFailed,
}

/// The result of one [`CatalogEffect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogOutcome {
    /// The catalogs were re-read; the resident catalogs now hold what the store had.
    ///
    /// It carries no revision: the store's revision reaches the domain as an external *fact*, and
    /// what a read owes back is only that the operation is over.
    CatalogRead { token: OperationToken<CatalogTag> },
    /// `object` is gone from the store. `existed` is `false` when it was already absent — the
    /// epic's "a trip member disappears before the delete commit" race, which is a *success* for
    /// the cascade (the goal state holds) and must not read as a failure.
    ObjectRemoved { token: OperationToken<CatalogTag>, object: CatalogObjectId, existed: bool },
    /// The operation failed.
    Failed { token: OperationToken<CatalogTag>, error: CatalogError },
    /// The executor abandoned the operation without completing it.
    Cancelled { token: OperationToken<CatalogTag> },
}

impl CatalogOutcome {
    /// The operation this outcome answers.
    pub fn token(&self) -> OperationToken<CatalogTag> {
        match self {
            CatalogOutcome::CatalogRead { token }
            | CatalogOutcome::ObjectRemoved { token, .. }
            | CatalogOutcome::Failed { token, .. }
            | CatalogOutcome::Cancelled { token } => *token,
        }
    }
}

/// The catalog domain's operation seam (#1438): admit an intent, issue the effects it implies, and
/// accept each answer.
///
/// It owns the two things a domain owner must own — its [`OperationToken`] and how many operations
/// may be in flight — plus the one ordering no single bounded operation can express: the **trip
/// cascade**, member routes first and the folder last.
impl CatalogState {
    /// Admit `intent`, or refuse it and hand it back.
    ///
    /// One refusal, and it is backpressure rather than failure: something is already in the slot —
    /// an intent waiting to become an effect, or a cascade still walking. Its producer keeps it, so
    /// "a busy catalog delays a delete, it never loses one" holds for every intent alike.
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
    /// **Deletions first, the owed re-read last**, and by construction rather than by a priority
    /// list: an admitted intent is a rider's delete or an expiry, and the re-read is only reached
    /// when there is none. It is also why the re-read never occupies the one intent slot — a second
    /// copy of it there is a second read (a store commit and the delete it caused would each get
    /// one), which is exactly what the single owed bit exists to prevent.
    ///
    /// The admitted intent is taken **before** the match, so no arm can leave the domain holding an
    /// intent it has already decided about. A cascade is the one arm that puts it back, and it
    /// advances [`cascade`](CatalogState::cascade) every time it does — the ordinal only ever grows
    /// and the stage list is bounded, so the walk always reaches the folder and releases the slot.
    pub(crate) fn next_effect(&mut self) -> Option<CatalogEffect> {
        if self.in_flight {
            return None;
        }
        let Some(intent) = self.pending.take() else {
            if !self.take_refresh_owed() {
                return None;
            }
            self.in_flight = true;
            return Some(CatalogEffect::ReadCatalog { token: self.ops.issue() });
        };
        let effect = match intent {
            CatalogIntent::DeleteRoute { id } | CatalogIntent::DeleteRide { id } => {
                CatalogEffect::RemoveObject { token: self.ops.issue(), object: id }
            }
            // The cascade, one member per operation. The trip's stage ids are already resident and
            // the `.obt` is untouched until the last step, so ordinal `n` names the same member on
            // every pass — the domain needs a cursor, not a member buffer.
            CatalogIntent::DeleteTrip { id } => {
                let ordinal = self.cascade.unwrap_or(0);
                match self.trip_member(id, ordinal) {
                    Some(member) => {
                        self.cascade = Some(ordinal.saturating_add(1));
                        self.pending = Some(intent); // the folder is still owed
                        CatalogEffect::RemoveObject { token: self.ops.issue(), object: member }
                    }
                    // Every member has had its turn: the folder itself is the last removal, and
                    // taking the intent above is what ends the cascade.
                    None => {
                        self.cascade = None;
                        CatalogEffect::RemoveObject { token: self.ops.issue(), object: id }
                    }
                }
            }
        };
        self.in_flight = true;
        Some(effect)
    }

    /// The member route id at stage `ordinal` of the resident trip `trip`, or `None` past its last
    /// stage (or when the trip is not resident at all, which ends the walk at the folder).
    fn trip_member(&self, trip: CatalogObjectId, ordinal: u8) -> Option<CatalogObjectId> {
        let trip = self.trips.iter().find(|t| t.id == trip)?;
        trip.stage_ids.get(usize::from(ordinal)).copied()
    }

    /// Consume the answer to a [`CatalogEffect`]. A stale token — a superseded operation, or a
    /// repeat of one already accounted for — changes nothing.
    ///
    /// The resident catalogs are not touched here: what is *in* the store reaches them through the
    /// refresh feed, and inventing a removal locally would make the two disagree until it did.
    ///
    /// A cascade step reads the same as any other answer, including a failed one. The walk advanced
    /// when the step went out, so a member the store refused is **left behind** rather than retried:
    /// the folder still goes, and the leftover route comes back as an unfiled row the rider can
    /// delete. Retrying instead would spin a cascade against a card that has stopped answering.
    pub(crate) fn apply_outcome(&mut self, outcome: CatalogOutcome) {
        if !self.ops.is_current(outcome.token()) {
            return;
        }
        self.ops.invalidate(); // terminal: a duplicate of this answer is no longer current
        self.in_flight = false;
        // A completed removal moved the store, so the resident catalogs are behind it; a read the
        // store could not answer is still owed, and nothing else would ever order it again. Both
        // `existed` verdicts arm: an object the store did not have may still be a resident row.
        //
        // A removal the store **refused** arms nothing — it changed nothing, retention re-queues
        // its own candidate, and a read per retry would walk the store for a store that did not
        // move. Neither does a cancellation.
        //
        // **A cascade needs no arm of its own.** Every member step arms this bit and none of them
        // can spend it: the walk keeps the `DeleteTrip` intent in `pending` until the folder, and
        // `next_effect` only reaches the owed read when nothing is pending. One bit, spent once,
        // after the folder.
        match outcome {
            CatalogOutcome::ObjectRemoved { .. } | CatalogOutcome::Failed { error: CatalogError::Unreadable, .. } => {
                self.refresh_owed = true
            }
            CatalogOutcome::Failed { .. } | CatalogOutcome::CatalogRead { .. } | CatalogOutcome::Cancelled { .. } => {}
        }
    }

    /// Note that the object store moved underneath us — the store-revision fact, read at stage 2.
    /// The fact is a level; the owed bit is what turns it into a read.
    pub(crate) fn note_store_moved(&mut self) {
        self.refresh_owed = true;
    }

    /// Take the owed re-read, if one is owed — [`next_effect`](CatalogState::next_effect)'s last
    /// arm.
    fn take_refresh_owed(&mut self) -> bool {
        core::mem::take(&mut self.refresh_owed)
    }
}

// Layout tripwires: an identity, a revision, a count — never a catalog.
const _: () = assert!(core::mem::size_of::<CatalogIntent>() <= 16, "a request with one identity");
const _: () = assert!(core::mem::size_of::<CatalogEffect>() <= 16, "a token and one identity");
const _: () = assert!(core::mem::size_of::<CatalogOutcome>() <= 16, "a token, an identity and a flag");
const _: () = assert!(core::mem::size_of::<CatalogError>() <= 1, "a verdict, not a report");

#[cfg(test)]
impl CatalogState {
    /// Assert the [`new`](CatalogState::new) boot state, field by field. The destructure is
    /// exhaustive, so a field added to the plan must state its boot value here too.
    pub(crate) fn assert_boot_state(&self) {
        let CatalogState {
            routes,
            route_ids,
            route_meta,
            trips,
            rides,
            ride_ids,
            ride_inventory,
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
            refresh_owed,
        } = self;
        assert!(routes.is_empty() && route_ids.is_empty() && route_meta.is_empty(), "no routes catalogued");
        assert!(trips.is_empty(), "no trips catalogued");
        assert!(rides.is_empty() && ride_ids.is_empty() && ride_inventory.is_empty(), "no rides catalogued");
        assert_eq!(ride_profile.cols(), Profile::EMPTY.cols(), "the ride-profile buffer is the empty line");
        assert!(!*ride_profile_present && ride_profile_for.is_none(), "no ride profile answered");
        assert!(ride_preview.is_empty() && ride_preview_for.is_none(), "no ride preview cached");
        assert!(nav_preview.is_empty() && nav_preview_route.is_none(), "no route-shape preview cached");
        assert!(
            [*source_revision, *ride_track_view, *nav_preview_view].iter().all(|r| *r == Revision::ZERO),
            "the derived key revisions start at zero — nothing committed, nothing invalidated"
        );
        assert!(detour_preview.is_empty() && detour_preview_route.is_none(), "no detour preview cached");
        assert!(pending.is_none() && cascade.is_none() && !*in_flight, "no catalog operation admitted or in flight");
        assert!(!*refresh_owed, "nothing has moved the store yet, so no re-read is owed");
        assert_eq!(format!("{ops:?}"), "TokenSource(0)", "no catalog operation has been issued");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    /// the finished walk orders (#1541) — which is the walk's own end marker.
    ///
    /// Bounded, and that is the point: the walk terminates because the ordinal only ever grows, so a
    /// cursor that stopped advancing is exactly the bug this helper must **report**. An unbounded
    /// loop would hang the suite on it instead of failing.
    fn drain_cascade(catalogs: &mut CatalogState) -> heapless::Vec<CatalogObjectId, 8> {
        let mut removed = heapless::Vec::new();
        for _ in 0..=removed.capacity() {
            let Some(effect) = catalogs.next_effect() else { return removed };
            let CatalogEffect::RemoveObject { token, object } = effect else { return removed };
            let _ = removed.push(object);
            catalogs.apply_outcome(CatalogOutcome::ObjectRemoved { token, object, existed: true });
        }
        panic!("the cascade did not reach the folder in {} steps — the cursor is not advancing", removed.capacity())
    }

    /// The cascade is the domain's ordering, made of the same bounded removal every other delete
    /// uses: each member route in stage order, then the folder — and nothing else is admitted while
    /// it walks, so the slot that delays a second delete delays one behind a cascade too.
    #[test]
    fn a_trip_cascade_removes_every_member_before_the_folder() {
        let mut catalogs = with_trip(50, &[10, 20, 30], &[10, 20, 30]);
        catalogs.admit_intent(CatalogIntent::DeleteTrip { id: 50 }).unwrap();

        // Mid-walk the slot is busy, and the refused intent goes back to its producer intact.
        let effect = catalogs.next_effect().expect("the first member");
        let later = CatalogIntent::DeleteRoute { id: 99 };
        assert_eq!(catalogs.admit_intent(later).unwrap_err().rejected, later, "handed back, never lost");
        let CatalogEffect::RemoveObject { token, object } = effect else { panic!("a removal") };
        assert_eq!(object, 10, "stage order, first member first");
        catalogs.apply_outcome(CatalogOutcome::ObjectRemoved { token, object, existed: true });

        assert_eq!(drain_cascade(&mut catalogs).as_slice(), &[20, 30, 50], "the rest, then the folder");
        catalogs.admit_intent(later).expect("the cascade released the slot");
    }

    /// **The same-numbering trap.** A trip and a route may carry the same id (a host that numbers
    /// its families from separate counters), and an id-only cascade would then take the route's
    /// file for the folder. The members are resolved through the *trip's own* stage list, so the
    /// route that merely shares the trip's number is never touched.
    #[test]
    fn a_route_that_shares_the_trips_id_is_not_cascaded() {
        // Trip 1 has one member, route 7. Route 1 exists too, and shares the trip's number.
        let mut catalogs = with_trip(1, &[7], &[7, 1]);
        catalogs.admit_intent(CatalogIntent::DeleteTrip { id: 1 }).unwrap();
        let removed = drain_cascade(&mut catalogs);
        assert_eq!(removed.as_slice(), &[7, 1], "the member, then the folder — and only twice");
        // The second removal is the folder's identity, which the executor resolves in the trip
        // namespace. Nothing in the walk ever named route 1 as a *member*.
        assert_eq!(removed.iter().filter(|&&id| id == 1).count(), 1, "route 1 is not a member of trip 1");
    }

    /// A member the store refused is left behind rather than retried: the ordinal advanced when the
    /// step went out, so the folder still goes and the leftover route reappears as an unfiled row.
    /// Retrying would spin the cascade against a card that has stopped answering.
    #[test]
    fn a_failed_member_step_does_not_stall_the_cascade() {
        let mut catalogs = with_trip(50, &[10, 20], &[10, 20]);
        catalogs.admit_intent(CatalogIntent::DeleteTrip { id: 50 }).unwrap();

        let effect = catalogs.next_effect().expect("the first member");
        catalogs.apply_outcome(CatalogOutcome::Failed { token: effect.token(), error: CatalogError::RemoveFailed });

        assert_eq!(drain_cascade(&mut catalogs).as_slice(), &[20, 50], "the walk moved on and finished");
    }

    /// **Ordinal stability.** The walk holds a cursor, not a copy of the member list, so the only
    /// thing that keeps ordinal `n` naming the same member is that nothing rewrites the list under
    /// it. Nothing does: a host re-feed reads the trip's own stage refs verbatim, and the cascade
    /// leaves the folder alone until its last step — so a member already removed is still named,
    /// as a dangling id, and the list is the one the walk started on.
    #[test]
    fn a_catalog_re_feed_mid_cascade_does_not_move_the_cursor() {
        let mut catalogs = with_trip(50, &[10, 20, 30], &[10, 20, 30]);
        catalogs.admit_intent(CatalogIntent::DeleteTrip { id: 50 }).unwrap();

        let effect = catalogs.next_effect().expect("the first member");
        let CatalogEffect::RemoveObject { token, object } = effect else { panic!("a removal") };
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

    // ==================== the owed re-read (#1541) ====================

    /// A completed removal orders a re-read: the store moved, so the resident catalogs are behind
    /// it. **Exactly one** — the owed bit is a bit, and taking it is what spends it.
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

            catalogs.apply_outcome(CatalogOutcome::CatalogRead { token: read.token() });
            assert!(catalogs.next_effect().is_none(), "and one — a second would walk the whole store again");
        }
    }

    /// The cascade orders **one** re-read, after the folder — never one per member. The cursor is
    /// `Some` for every member step and `None` by the time the folder's answer arrives, which is
    /// the whole of the rule.
    #[test]
    fn a_cascade_orders_one_re_read_after_the_folder() {
        let mut catalogs = with_trip(50, &[10, 20, 30], &[10, 20, 30]);
        catalogs.admit_intent(CatalogIntent::DeleteTrip { id: 50 }).unwrap();

        // `None` is the re-read; every other step names the object it removed.
        let mut steps: heapless::Vec<Option<CatalogObjectId>, 8> = heapless::Vec::new();
        for _ in 0..=steps.capacity() {
            let Some(effect) = catalogs.next_effect() else { break };
            match effect {
                CatalogEffect::RemoveObject { token, object } => {
                    let _ = steps.push(Some(object));
                    catalogs.apply_outcome(CatalogOutcome::ObjectRemoved { token, object, existed: true });
                }
                CatalogEffect::ReadCatalog { token } => {
                    let _ = steps.push(None);
                    catalogs.apply_outcome(CatalogOutcome::CatalogRead { token });
                }
            }
        }
        assert_eq!(
            steps.as_slice(),
            &[Some(10), Some(20), Some(30), Some(50), None],
            "every member, then the folder, then exactly one re-read"
        );
    }

    /// A removal the store **refused** changed nothing, so it orders nothing. Retention re-queues
    /// its own candidate; a read per retry would walk the store for a store that did not move.
    #[test]
    fn a_removal_the_store_refused_orders_no_re_read() {
        let mut catalogs = CatalogState::new();
        catalogs.admit_intent(CatalogIntent::DeleteRoute { id: 10 }).unwrap();
        let effect = catalogs.next_effect().expect("the removal");
        catalogs.apply_outcome(CatalogOutcome::Failed { token: effect.token(), error: CatalogError::RemoveFailed });
        assert!(catalogs.next_effect().is_none(), "a refused removal moved nothing, so it orders nothing");
    }

    /// The placement path must land exactly the state the by-value path builds.
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
