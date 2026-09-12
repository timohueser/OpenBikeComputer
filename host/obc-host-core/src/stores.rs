//! The in-memory store family — for a host without a filesystem (the browser demo; also handy in
//! tests). Each mirrors the surface of `obc-sim`'s folder-backed twin, so host code drives either
//! shape identically; nothing above the store (`obc-app`, `obc-render`) knows the difference.

use crate::{RideRepository, RouteRepository, TrackRepository};
use obc_app::catalog_state::CatalogError;
use obc_app::recorder::RideClose;
use obc_app::{CatalogObjectId, RideEntry, RideSummary};
use obc_formats::io::SliceSource;
use obc_route::{Profile, RideStats, RouteSummary};

/// The in-memory route store's reserved id for the computed nav route (out of the small
/// positional band the seeded catalog uses), so a re-plan replaces the previous computed route
/// in place — the twin of the folder store's overwrite-in-place `_nav.obcr`.
pub const MEM_NAV_ID: CatalogObjectId = CatalogObjectId::MAX;

/// An in-memory route store: a fixed seeded catalog (e.g. routes compiled into the wasm binary)
/// plus the reserved slot the router's computed route lands in.
pub struct MemRouteStore {
    catalog: Vec<RouteSummary>,
    ids: Vec<CatalogObjectId>,
    bytes: Vec<Vec<u8>>,
    active: Option<usize>,
}

impl MemRouteStore {
    /// Seed the catalog from `routes` (each a complete `.obcr`). Unparsable entries are skipped —
    /// a bad embed shows as a missing menu row, not a crash. The catalog is fixed, so positional
    /// ids are session-stable.
    pub fn new(routes: &[&[u8]]) -> Self {
        let mut s = MemRouteStore { catalog: Vec::new(), ids: Vec::new(), bytes: Vec::new(), active: None };
        for route in routes {
            if let Ok(sum) = RouteSummary::read(&SliceSource(route)) {
                s.catalog.push(sum);
                s.ids.push(s.ids.len() as CatalogObjectId);
                s.bytes.push(route.to_vec());
            }
        }
        s
    }

    /// The route catalog (paired entries), for [`App::set_routes_with_ids`](obc_app::App::set_routes_with_ids).
    pub fn catalog(&self) -> &[RouteSummary] {
        &self.catalog
    }

    /// Each catalog entry's id, parallel to [`catalog`](MemRouteStore::catalog) (fixed in memory).
    pub fn ids(&self) -> &[CatalogObjectId] {
        &self.ids
    }

    /// Make the active route match `want` — the in-memory twin serves bytes by index, so this is
    /// just a bounds-checked assignment. Returns whether the active binding changed (the reparse
    /// signal). Cheap to call every frame.
    pub fn sync_active(&mut self, want: Option<usize>) -> bool {
        let next = want.filter(|&i| i < self.bytes.len());
        let changed = next != self.active;
        self.active = next;
        changed
    }

    /// See the folder-backed twin: force a re-read after the nav bytes are replaced under an
    /// unchanged index (here only the reset matters — the next `sync_active` re-binds).
    pub fn invalidate_active(&mut self) {
        self.active = None;
    }

    /// A [`ByteSource`](obc_formats::io::ByteSource) over the active route's bytes, for opening a
    /// [`RouteReader`](obc_route::RouteReader) to stream geometry from.
    pub fn active_source(&self) -> Option<SliceSource<'_>> {
        self.active.and_then(|i| self.bytes.get(i)).map(|b| SliceSource(b.as_slice()))
    }

    /// Delete the route with id `id` (the on-device hold-to-delete, epic #447 P6). `Ok(true)` = removed, `Ok(false)` = absent. The id isn't re-issued (the seeded catalog is fixed and positional).
    pub fn delete_by_id(&mut self, id: CatalogObjectId) -> Result<bool, CatalogError> {
        let Some(pos) = self.ids.iter().position(|&x| x == id) else { return Ok(false) };
        self.catalog.remove(pos);
        self.ids.remove(pos);
        self.bytes.remove(pos);
        if self.active == Some(pos) {
            self.active = None;
        } else if let Some(a) = self.active.as_mut() {
            if *a > pos {
                *a -= 1;
            }
        }
        Ok(true)
    }
}

impl RouteRepository for MemRouteStore {
    fn catalog(&self) -> &[RouteSummary] {
        self.catalog()
    }

    fn ids(&self) -> &[CatalogObjectId] {
        self.ids()
    }

    fn delete_by_id(&mut self, id: CatalogObjectId) -> Result<bool, CatalogError> {
        self.delete_by_id(id)
    }

    /// The in-memory twin of the folder store's reserved `_nav.obcr` write: replace (or append)
    /// the computed route under the fixed [`MEM_NAV_ID`] and return it.
    fn write_nav_route(&mut self, bytes: &[u8]) -> Option<CatalogObjectId> {
        let sum = RouteSummary::read(&SliceSource(bytes)).ok()?;
        match self.ids.iter().position(|&id| id == MEM_NAV_ID) {
            Some(pos) => {
                self.catalog[pos] = sum;
                self.bytes[pos] = bytes.to_vec();
            }
            None => {
                self.catalog.push(sum);
                self.ids.push(MEM_NAV_ID);
                self.bytes.push(bytes.to_vec());
            }
        }
        Some(MEM_NAV_ID)
    }

    fn sync_active(&mut self, want: Option<usize>) -> bool {
        self.sync_active(want)
    }

    fn active_source(&self) -> Option<SliceSource<'_>> {
        self.active_source()
    }

    fn invalidate_active(&mut self) {
        self.invalidate_active()
    }
}

/// An in-memory ride store: a fixed demo catalog so the Rides screen renders (#454). Hold-to-delete
/// removes rows for the session; nothing is ever written.
pub struct MemRideStore {
    catalog: Vec<RideEntry>,
}

impl MemRideStore {
    /// Seed the catalog (newest first, as [`App::set_rides`](obc_app::App::set_rides) expects).
    /// Positional ids — the catalog is fixed, so they're session-stable — carved out of
    /// [`RIDE_ID_BASE`](crate::RIDE_ID_BASE) so a ride and a route can never share an identity the
    /// namespace-free `CatalogEffect::RemoveObject` would confuse.
    pub fn new(catalog: Vec<RideSummary>) -> Self {
        let catalog = catalog
            .into_iter()
            .enumerate()
            .map(|(i, summary)| RideEntry { id: crate::RIDE_ID_BASE + i as CatalogObjectId, summary })
            .collect();
        MemRideStore { catalog }
    }

    /// The ride catalog (paired entries), for [`App::set_rides`](obc_app::App::set_rides).
    pub fn catalog(&self) -> &[RideEntry] {
        &self.catalog
    }

    /// Delete the ride with id `id` (the hold-to-delete footer). `Ok(true)` = removed, `Ok(false)` = absent.
    pub fn delete_by_id(&mut self, id: obc_app::CatalogObjectId) -> Result<bool, CatalogError> {
        let Some(pos) = self.catalog.iter().position(|entry| entry.id == id) else { return Ok(false) };
        self.catalog.remove(pos);
        Ok(true)
    }
}

impl RideRepository for MemRideStore {
    fn catalog(&self) -> &[RideEntry] {
        self.catalog()
    }

    fn delete_by_id(&mut self, id: obc_app::CatalogObjectId) -> Result<bool, CatalogError> {
        self.delete_by_id(id)
    }

    /// Memory rides have no stored track. The keyed failure parks the empty detail.
    fn fill_track(&self, _id: obc_app::CatalogObjectId, _profile: &mut Profile) -> Option<Vec<(i32, i32)>> {
        None
    }
}

/// An in-memory track store: no filesystem, so no on-disk ride object — the breadcrumb and the ride
/// totals are the app's own. It takes the samples it is handed and keeps none of them, and mirrors
/// whether a ride is active so
/// `is_recording()` stays honest, and numbers the rides it closes so a finalize can answer with an
/// identity like every other store.
#[derive(Default)]
pub struct MemTrackStore {
    recording: bool,
    /// The next ride identity. The store keeps no bytes, but a finalize still has to name what it
    /// closed — an answer of "no identity" is how a *failure* is reported, and this one succeeded.
    next_id: CatalogObjectId,
}

impl MemTrackStore {
    pub fn new() -> Self {
        MemTrackStore::default()
    }

    pub fn is_recording(&self) -> bool {
        self.recording
    }
}

impl TrackRepository for MemTrackStore {
    /// Mirror the folder-backed store's recording flag without touching a filesystem. `name` is
    /// irrelevant with no on-disk log.
    fn open(&mut self, _session: u32, _name: Option<&str>) {
        self.recording = true;
    }

    fn finalize(&mut self, _stats: RideStats) -> RideClose {
        if !self.recording {
            return RideClose::Nothing;
        }
        self.recording = false;
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        RideClose::Committed(id)
    }

    fn discard(&mut self) -> bool {
        self.recording = false;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A junk seed is skipped (not a crash, not a phantom row), and the store stays usable.
    #[test]
    fn junk_route_seed_is_skipped() {
        let s = MemRouteStore::new(&[b"not an obcr".as_slice()]);
        assert!(s.catalog().is_empty() && s.ids().is_empty());
    }

    /// `sync_active` bounds-checks, and `delete_by_id` keeps the active binding on the same
    /// route (or drops it when that route was the one deleted) — the remap the Rides/Route
    /// screens rely on.
    #[test]
    fn delete_remaps_the_active_index() {
        // Seeding real summaries needs real OBCR bytes; the index/active mechanics are what's
        // under test, so build the store shape directly (summary content is irrelevant to them).
        let dummy = RouteSummary {
            name: Default::default(),
            distance_km: 0,
            climb_m: 0,
            bbox: obc_map_scene::BBox { min_lon: 0, min_lat: 0, max_lon: 0, max_lat: 0 },
            start_lon: 0,
            start_lat: 0,
        };
        let mut s = MemRouteStore {
            catalog: vec![dummy.clone(), dummy.clone(), dummy],
            ids: vec![0, 1, 2],
            bytes: vec![vec![0], vec![1], vec![2]],
            active: Some(2),
        };
        assert_eq!(s.delete_by_id(9), Ok(false), "unknown id is a no-op");
        assert_eq!(s.delete_by_id(0), Ok(true));
        assert_eq!(s.active, Some(1), "active shifts down past a deletion before it");
        assert_eq!(s.delete_by_id(2), Ok(true));
        assert_eq!(s.active, None, "deleting the active route drops the binding");
    }

    /// The lifecycle the shared suite pins, on the memory store: opening records, and either close
    /// stops. The memory store keeps no bytes, so its finalize still has to *name* what it closed —
    /// "nothing" is how "there was no ride" is reported, and it is not a failure.
    #[test]
    fn the_memory_store_mirrors_the_ride_and_names_what_it_closes() {
        let mut t = MemTrackStore::new();
        crate::conformance::track_lifecycle(&mut t);
        assert!(!t.is_recording());
    }
}
