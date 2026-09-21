//! The in-memory store family — for a host without a filesystem (the browser demo; also handy in
//! tests). Each mirrors the surface of `obc-sim`'s folder-backed twin, so host code drives either
//! shape identically; nothing above the store (`obc-app`, `obc-render`) knows the difference.

use crate::{RideRepository, TrackRepository};
use obc_app::catalog_state::CatalogError;
use obc_app::recorder::RideClose;
use obc_app::{CatalogObjectId, RideEntry, RideSummary};
use obc_route::{Profile, RideStats};

/// An in-memory ride store: a fixed demo catalog so the Rides screen renders. Hold-to-delete
/// removes rows for the session; nothing is ever written.
pub struct MemRideStore {
    catalog: Vec<RideEntry>,
}

impl MemRideStore {
    /// Seed the catalog (newest first, as [`App::set_rides`](obc_app::App::set_rides) expects).
    /// Positional ids — the catalog is fixed, so they're session-stable — carved out of
    /// [`RIDE_ID_BASE`](crate::RIDE_ID_BASE)'s fixture band. Deletion also retains the Ride kind.
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
/// totals are the app's own. It takes the samples it is handed and keeps none of them, mirrors
/// whether a ride is active so `is_recording()` stays honest, and numbers the rides it closes so a
/// finalize can answer with an identity like every other store.
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
    fn open(&mut self, _session: u32, _name: Option<&str>, _now_ms: u32) -> bool {
        self.recording = true;
        true
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

    fn discard(&mut self) -> Result<(), obc_app::recorder::RecorderError> {
        self.recording = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
