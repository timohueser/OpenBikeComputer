//! [`CatalogState`] — the resident route / ride / trip catalogs, keyed by durable object ids.
//!
//! One component owns every id ↔ summary pairing and every piece of state keyed by catalog
//! *identity* (the #450 contract): the route and ride catalogs with their durable ids, the trip
//! folders resolving stage ids against the route catalog, and the host-filled view caches (the
//! viewed ride's profile/preview, the route overview's shape preview) whose staleness keys are
//! catalog indices that must follow identity across a live rescan.
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
    /// The ride index [`ride_profile`](CatalogState::ride_profile) was **answered** for (a failed
    /// fill parks `None` under the same key so a dead file isn't re-streamed), remapped by
    /// identity across rescans like every held ride index.
    ride_profile_for: Option<usize>,
    /// The viewed ride's decimated recorded-track shape polyline (#678 rework 3), host-filled in
    /// the same drain as the profile.
    ride_preview: heapless::Vec<(i32, i32), NAV_PREVIEW_MAX>,
    /// The ride index the [`ride_preview`](CatalogState::ride_preview) was handed in for — its
    /// staleness key, remapped like the profile key.
    ride_preview_for: Option<usize>,
    /// The Route overview's decimated route-shape preview polyline (#685 §4), host-decimated and
    /// handed in via [`set_nav_preview`](CatalogState::set_nav_preview).
    nav_preview: heapless::Vec<(i32, i32), NAV_PREVIEW_MAX>,
    /// The route index the [`nav_preview`](CatalogState::nav_preview) was handed in for — the
    /// staleness key (the render gates on it so an old plan's shape can never draw under a
    /// different route). Cleared when a plan commits so every plan starts preview-less.
    nav_preview_route: Option<usize>,
    /// The **detour preview** polyline (#882): the planned-but-uncommitted detour's decimated
    /// shape, drawn by the Detour preview screen *over* the still-active original route.
    /// Host-filled when the detour plan completes; cleared on commit, cancel, or route change.
    detour_preview: heapless::Vec<(i32, i32), NAV_PREVIEW_MAX>,
    /// The route index the [`detour_preview`](CatalogState::detour_preview) was planned against —
    /// its staleness key (a route swap or rescan mid-preview blanks the overlay rather than
    /// drawing a stale detour over different geometry).
    detour_preview_route: Option<usize>,
}

impl CatalogState {
    /// Empty catalogs, nothing cached — the boot state.
    pub(crate) const fn new() -> Self {
        CatalogState {
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
            detour_preview: heapless::Vec::new(),
            detour_preview_route: None,
        }
    }

    /// Initialize `slot` **in place** to the [`new`](CatalogState::new) state — the placement path
    /// (the catalogs are several KB; nothing here may form a by-value `CatalogState` on the
    /// stack). Same field-by-field `addr_of_mut!` discipline as [`App::init_idle`], with the same
    /// trailing exhaustiveness guard.
    ///
    /// # Safety
    /// `slot` must be valid, aligned, exclusively owned, and writable for a full `CatalogState`.
    pub(crate) unsafe fn init_in_place(slot: *mut Self) {
        use core::ptr::addr_of_mut;
        // SAFETY: caller's contract; every field is written exactly once before any read.
        unsafe {
            addr_of_mut!((*slot).routes).write(Catalog::new());
            addr_of_mut!((*slot).route_ids).write(heapless::Vec::new());
            addr_of_mut!((*slot).route_meta).write(heapless::Vec::new());
            addr_of_mut!((*slot).trips).write(Trips::new());
            addr_of_mut!((*slot).rides).write(RideCatalog::new());
            addr_of_mut!((*slot).ride_ids).write(heapless::Vec::new());
            addr_of_mut!((*slot).ride_inventory).write(heapless::Vec::new());
            addr_of_mut!((*slot).ride_profile).write(Profile::EMPTY);
            addr_of_mut!((*slot).ride_profile_present).write(false);
            addr_of_mut!((*slot).ride_profile_for).write(None);
            addr_of_mut!((*slot).ride_preview).write(heapless::Vec::new());
            addr_of_mut!((*slot).ride_preview_for).write(None);
            addr_of_mut!((*slot).nav_preview).write(heapless::Vec::new());
            addr_of_mut!((*slot).nav_preview_route).write(None);
            addr_of_mut!((*slot).detour_preview).write(heapless::Vec::new());
            addr_of_mut!((*slot).detour_preview_route).write(None);
            // Exhaustiveness guard: a field added to `CatalogState` fails to compile here until
            // its `addr_of_mut!(...).write(...)` is added above (see `App::init_idle`).
            let CatalogState {
                routes: _,
                route_ids: _,
                route_meta: _,
                trips: _,
                rides: _,
                ride_ids: _,
                ride_inventory: _,
                ride_profile: _,
                ride_profile_present: _,
                ride_profile_for: _,
                ride_preview: _,
                ride_preview_for: _,
                nav_preview: _,
                nav_preview_route: _,
                detour_preview: _,
                detour_preview_route: _,
            } = &*slot;
        }
    }

    // ---- route catalog ----

    /// The resident route summaries (what `Ctx`/`Render` hand the screens).
    pub(crate) fn routes(&self) -> &[RouteSummary] {
        &self.routes
    }

    /// Each route's durable id, pairwise with [`routes`](CatalogState::routes).
    pub(crate) fn route_ids(&self) -> &[CatalogObjectId] {
        &self.route_ids
    }

    /// The paired `{id, summary}` at catalog index `idx`, or `None` out of range — the entry-typed
    /// read (the id and summary can't be picked from mismatched rows).
    pub(crate) fn route_entry(&self, idx: usize) -> Option<RouteEntry<'_>> {
        Some(RouteEntry { id: *self.route_ids.get(idx)?, summary: self.routes.get(idx)? })
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
        // The view caches follow their subject's identity (identity survives → the resident
        // profile moves with it, no re-stream; vanished → the buffer drops).
        self.ride_profile_for = self.ride_profile_for.and_then(|i| self.remap_ride(&old_ids, i));
        if self.ride_profile_for.is_none() {
            self.ride_profile_present = false; // the profiled ride vanished (or none was profiled)
        }
        self.ride_preview_for = self.ride_preview_for.and_then(|i| self.remap_ride(&old_ids, i));
        if self.ride_preview_for.is_none() {
            self.ride_preview.clear(); // the previewed ride vanished (or none was previewed)
        }
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

    // ---- identity-keyed view caches ----

    /// Park the host's ride-track answer in the single resident ride-profile buffer, keyed to
    /// `viewed_ride` (`None` profile = the stream failed; the cue stops re-firing).
    pub(crate) fn set_ride_profile(&mut self, profile: Option<Profile>, viewed_ride: Option<usize>) {
        if let Some(profile) = profile {
            self.ride_profile = profile;
            self.ride_profile_present = true;
        } else {
            self.ride_profile_present = false;
        }
        self.ride_profile_for = viewed_ride;
    }

    /// Borrow the one resident profile buffer for an in-place host fill. The matching
    /// [`finish_ride_profile_fill`](Self::finish_ride_profile_fill) call publishes or rejects it.
    pub(crate) fn begin_ride_profile_fill(&mut self) -> &mut Profile {
        self.ride_profile_present = false;
        &mut self.ride_profile
    }

    /// Complete an in-place profile fill, including a failed answer so the level-triggered request
    /// does not grind on malformed media every pass.
    pub(crate) fn finish_ride_profile_fill(&mut self, valid: bool, viewed_ride: Option<usize>) {
        self.ride_profile_present = valid;
        self.ride_profile_for = viewed_ride;
    }

    /// The resident ride profile **iff** it was answered for `viewed_ride` (a `None` key never
    /// matches — the buffer is only reachable through the identity it was filled for).
    pub(crate) fn ride_profile_for(&self, viewed_ride: Option<usize>) -> Option<&Profile> {
        (self.ride_profile_present && self.ride_profile_for.is_some() && self.ride_profile_for == viewed_ride)
            .then_some(&self.ride_profile)
    }

    /// Whether the ride-profile buffer is answered for `viewed_ride` — the derived
    /// [`LoadRideTrack`](crate::host::HostCommand::LoadRideTrack) cue's "already answered" half
    /// (a recorded failure counts as answered, so a dead file never grinds).
    pub(crate) fn ride_profile_answered_for(&self, viewed_ride: usize) -> bool {
        self.ride_profile_for == Some(viewed_ride)
    }

    /// Hand in the viewed ride's decimated track-shape polyline (≤ [`NAV_PREVIEW_MAX`] points,
    /// more truncated), keyed to `viewed_ride`.
    pub(crate) fn set_ride_preview(&mut self, pts: &[(i32, i32)], viewed_ride: Option<usize>) {
        self.ride_preview.clear();
        for &p in pts.iter().take(NAV_PREVIEW_MAX) {
            let _ = self.ride_preview.push(p);
        }
        self.ride_preview_for = viewed_ride;
    }

    /// The ride-shape preview for `viewed_ride`, or the empty slice when missing/stale — the
    /// screens draw whatever this hands them, so a stale shape is unreachable.
    pub(crate) fn ride_preview_for(&self, viewed_ride: Option<usize>) -> &[(i32, i32)] {
        if self.ride_preview_for.is_some() && self.ride_preview_for == viewed_ride {
            &self.ride_preview
        } else {
            &[]
        }
    }

    /// Drop the ride profile + preview the moment they stop matching the viewed ride (#680): the
    /// detail exited or moved subjects. Filling is the host's; only the drop lives here.
    pub(crate) fn drop_stale_ride_views(&mut self, viewed_ride: Option<usize>) {
        if self.ride_profile_for != viewed_ride {
            self.ride_profile_present = false;
            self.ride_profile_for = None;
        }
        if self.ride_preview_for != viewed_ride {
            self.ride_preview.clear();
            self.ride_preview_for = None;
        }
    }

    /// Hand in the previewed route's decimated shape polyline (#685 §4), keyed to `active_route`.
    pub(crate) fn set_nav_preview(&mut self, pts: &[(i32, i32)], active_route: Option<usize>) {
        self.nav_preview.clear();
        for &p in pts.iter().take(NAV_PREVIEW_MAX) {
            let _ = self.nav_preview.push(p);
        }
        self.nav_preview_route = active_route;
    }

    /// The route-shape preview for `active_route`, or the empty slice when missing/stale.
    pub(crate) fn nav_preview_for(&self, active_route: Option<usize>) -> &[(i32, i32)] {
        if self.nav_preview_route.is_some() && self.nav_preview_route == active_route {
            &self.nav_preview
        } else {
            &[]
        }
    }

    /// Whether the nav preview is **stale** for `active_route` — the derived
    /// [`RefreshNavPreview`](crate::host::HostCommand::RefreshNavPreview) cue's data half (the
    /// screen half — is an overview up? — is the UI's).
    pub(crate) fn nav_preview_stale(&self, active_route: Option<usize>) -> bool {
        self.nav_preview_route != active_route
    }

    /// Clear the nav preview and its key — every committed plan starts preview-less (#685 §4), so
    /// a re-route's old shape can never survive into the new overview.
    pub(crate) fn clear_nav_preview(&mut self) {
        self.nav_preview.clear();
        self.nav_preview_route = None;
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
