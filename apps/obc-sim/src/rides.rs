//! The simulator's file-backed stand-in for the flat Ride catalog.
//!
//! The **host** side of the Rides screen's catalog: stored rides live as `ride-{id}.obcr` files in
//! the tracks folder (written by [`TrackStore`](crate::track::TrackStore) at Finish). That namespace
//! is explicitly a desktop fixture convention, not a device filename. The device records and lists
//! flat objects. Synced stamps are process-local until #1398 supplies the shared flat ride-domain
//! metadata boundary.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use obc_app::catalog_state::CatalogError;
use obc_app::{CatalogObjectId, RideEntry, RideSummary};
use obc_formats::io::SliceSource;
use obc_route::{ride_track_into, Profile, RideInfo};

/// The folder-backed ride store: paired ride entries (newest first) and their fixture paths.
pub struct RideStore {
    dir: PathBuf,
    catalog: Vec<RideEntry>,
    paths: Vec<PathBuf>,
    synced: HashMap<CatalogObjectId, u32>,
}

impl RideStore {
    /// Open and scan the tracks folder (a missing folder scans to an empty catalog).
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        let mut s = RideStore { dir: dir.into(), catalog: Vec::new(), paths: Vec::new(), synced: HashMap::new() };
        s.rescan();
        s
    }

    /// The ride catalog (paired entries, newest first), for [`App::set_rides`](obc_app::App::set_rides).
    pub fn catalog(&self) -> &[RideEntry] {
        &self.catalog
    }

    /// Re-read the folder's `ride-{id}.obcr` files into the catalog (newest first by `start_time`),
    /// with larger durable IDs first for equal times. Each carries this process's synced fact.
    pub fn rescan(&mut self) {
        self.catalog.clear();
        self.paths.clear();
        let mut rows: Vec<(CatalogObjectId, PathBuf, RideSummary)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                let p = e.path();
                let Some(id) = fixture_object_id_in(&p) else { continue };
                if let Ok(bytes) = std::fs::read(&p) {
                    if let Ok(info) = RideInfo::read(&SliceSource(&bytes)) {
                        let synced_at = self.synced.get(&id).copied().unwrap_or(0);
                        let sum = RideSummary::from_info(&info, synced_at != 0, synced_at);
                        rows.push((id, p, sum));
                    }
                }
            }
        }
        rows.sort_by_key(|r| std::cmp::Reverse((r.2.start_time, r.0)));
        for (id, path, sum) in rows {
            self.paths.push(path);
            self.catalog.push(RideEntry { id, summary: sum });
        }
    }

    /// Remove the ride file and its process-local synced flag, then refresh the catalog.
    /// `Ok(true)` means removed, `Ok(false)` means absent, and `Err` means storage failure.
    pub fn delete_by_id(&mut self, id: CatalogObjectId) -> Result<bool, CatalogError> {
        let Some(pos) = self.catalog.iter().position(|entry| entry.id == id) else { return Ok(false) };
        let path = self.paths[pos].clone();
        let existed = match std::fs::remove_file(&path) {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => return Err(CatalogError::RemoveFailed),
        };
        self.synced.remove(&id);
        self.rescan();
        Ok(existed)
    }

    /// Read one stored ride into the keyed detail's profile and preview. Unknown or unreadable
    /// objects fail the whole answer; no partial profile or preview is published.
    pub fn fill_track(&self, id: CatalogObjectId, profile: &mut Profile) -> Option<Vec<(i32, i32)>> {
        let pos = self.catalog.iter().position(|entry| entry.id == id)?;
        let bytes = std::fs::read(&self.paths[pos]).ok()?;
        let mut preview = Default::default();
        ride_track_into::<{ obc_app::NAV_PREVIEW_MAX }>(&SliceSource(&bytes), profile, &mut preview).ok()?;
        Some(preview.as_slice().to_vec())
    }

    /// Mark ride `id` as synced for this simulator process. The first nonzero stamp wins.
    pub fn mark_synced(&mut self, id: CatalogObjectId, utc: u32) {
        if utc != 0 && !self.synced.contains_key(&id) {
            self.synced.insert(id, utc);
            self.rescan();
        }
    }

    /// Start the process-local retention countdown if this ride has no stamp yet.
    pub fn stamp_synced_at(&mut self, id: CatalogObjectId, utc: u32) {
        if utc != 0 && !self.synced.contains_key(&id) {
            self.synced.insert(id, utc);
            self.rescan();
        }
    }
}

/// The shared dispatcher ([`obc_host_core::HostLoop`]) drives the ride catalog + per-ride track
/// reads through this trait — the same delete/re-feed/track-fill sequencing the board runs.
impl obc_host_core::RideRepository for RideStore {
    fn catalog(&self) -> &[RideEntry] {
        self.catalog()
    }
    fn delete_by_id(&mut self, id: CatalogObjectId) -> Result<bool, CatalogError> {
        self.delete_by_id(id)
    }
    fn fill_track(&self, id: CatalogObjectId, profile: &mut Profile) -> Option<Vec<(i32, i32)>> {
        self.fill_track(id, profile)
    }
    /// A `Save` just wrote a fresh desktop ride object; re-scan so it appears in the Rides menu live.
    fn refresh(&mut self) {
        self.rescan();
    }
    fn stamp_synced_at(&mut self, id: CatalogObjectId, utc: u32) {
        self.stamp_synced_at(id, utc)
    }
}

/// The object id a desktop `ride-{id}.obcr` fixture path names, or `None` for every file this store
/// cannot name unambiguously. There is deliberately no compatibility parser for historical device
/// filenames.
///
/// The filename number is carried into [`RIDE_ID_BASE`](obc_host_core::RIDE_ID_BASE)'s band, so a
/// ride and a route can never share an object identity: the typed store executor removes an object
/// by identity alone (`CatalogEffect::RemoveObject` is namespace-free, like the flat store it was
/// written for), and this folder store numbers each family from zero. Only the id moves — the file
/// on disk keeps its plain name.
///
/// A filename number the band cannot hold is therefore **not listed at all**, rather than listed
/// under an id that collides with a route or a trip: the allocator below never mints one, so the
/// only way to see this is a hand-written fixture, and an absent row is the honest answer to a file
/// whose identity this store cannot state.
fn fixture_object_id_in(p: &Path) -> Option<CatalogObjectId> {
    p.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("ride-"))
        .and_then(|n| n.strip_suffix(".obcr"))
        .and_then(|n| n.parse::<CatalogObjectId>().ok())
        .and_then(|n| n.checked_add(obc_host_core::RIDE_ID_BASE))
        .filter(|&id| id < obc_host_core::TRIP_ID_BASE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::TrackStore;
    use obc_host_core::TrackRepository;
    use obc_ports::TrackPoint;
    use obc_route::RideStats;

    /// Record and save one short ride into `dir` (session `id`), producing a real v3 desktop object
    /// with elevation + geometry — the folder ride store's conformance fixture, built through the
    /// same public `TrackStore` path the app drives.
    fn record_ride(dir: &Path, session: u32, name: &str) {
        let mut ts = TrackStore::open(dir);
        ts.open(session, Some(name));
        for k in 0..6u32 {
            assert!(ts.append(TrackPoint {
                lon: 8_000_000 + k as i32 * 200,
                lat: 46_000_000 + k as i32 * 200,
                ele: 1000 + k as i16 * 10,
                t_ms: k * 1000,
                segment_start: k == 0,
                hr: None,
                cadence: None,
                power: None,
            }));
        }
        let stats = RideStats {
            distance_m: 500,
            moving_time_s: 300,
            avg_speed_cms: 166,
            climb_m: 50,
            unix_at_anchor: 1_700_000_000,
            anchor_ms: 0,
            clock_trusted: true,
            avg_hr: None,
            max_hr: None,
            avg_cadence: None,
            avg_power: None,
            max_power: None,
        };
        assert!(
            matches!(ts.finalize(stats), obc_app::recorder::RideClose::Committed(_)),
            "the fixture ride is committed"
        );
    }

    /// The **folder-backed** ride store passes the shared `obc-host-core` conformance suite: unknown
    /// ids never read, a *known* ride yields its recorded profile + preview (`expects_track = true`,
    /// unlike the trackless memory store), and delete retires the id.
    #[test]
    fn folder_ride_store_passes_the_conformance_suite() {
        let dir = obcm_testkit::scratch::scratch_dir("obc-ride-conf", "suite");
        record_ride(&dir, 1, "Ride One");
        record_ride(&dir, 2, "Ride Two");

        let mut store = RideStore::open(&dir);
        assert_eq!(store.catalog().len(), 2, "two saved rides scanned");
        assert_eq!(
            store.catalog()[0].summary.start_time,
            store.catalog()[1].summary.start_time,
            "the records tie on time"
        );
        for _ in 0..2 {
            assert_eq!(
                store.catalog().iter().map(|entry| entry.id).collect::<Vec<_>>(),
                [obc_host_core::RIDE_ID_BASE + 1, obc_host_core::RIDE_ID_BASE]
            );
            assert_eq!(
                store.catalog().iter().map(|r| r.summary.name.as_str()).collect::<Vec<_>>(),
                ["Ride Two", "Ride One"]
            );
            store.rescan();
        }
        obc_host_core::conformance::ride_repository_suite(&mut store, true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn desktop_fixture_names_are_full_width_and_carry_the_ride_id_band() {
        const BASE: CatalogObjectId = obc_host_core::RIDE_ID_BASE;
        assert_eq!(fixture_object_id_in(Path::new("ride-42.obcr")), Some(BASE + 42));
        assert_eq!(
            fixture_object_id_in(Path::new("ride-0.obcr")),
            Some(BASE),
            "the band is what separates it from route 0"
        );
        // A number the band cannot hold is not an object this store can name unambiguously, so it
        // is not listed at all — the allocator never mints one (`allocate_fixture_object_id`).
        assert_eq!(fixture_object_id_in(Path::new("ride-18446744073709551615.obcr")), None);
        for unrelated in ["ride-42.bin", "other-42.obcr", "ride-x.obcr"] {
            assert_eq!(fixture_object_id_in(Path::new(unrelated)), None, "unrelated fixture {unrelated} is ignored");
        }
    }

    #[test]
    fn deletion_keeps_io_failure_distinct_from_absence() {
        let dir = obcm_testkit::scratch::scratch_dir("obc-ride-conf", "delete-failure");
        record_ride(&dir, 1, "Ride");
        let mut store = RideStore::open(&dir);
        let id = store.catalog[0].id;
        let path = store.paths[0].clone();
        let before = store.catalog.clone();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        assert_eq!(store.delete_by_id(id), Err(CatalogError::RemoveFailed));
        assert_eq!(store.catalog, before, "a failed unlink keeps the catalog intact");
        assert!(path.is_dir(), "a failed unlink does not remove the obstruction");

        std::fs::remove_dir(&path).unwrap();
        assert_eq!(store.delete_by_id(id), Ok(false), "the externally removed object is absent");
        assert!(!store.catalog.iter().any(|entry| entry.id == id), "absence refreshes the stale catalog");
        std::fs::write(&path, bytes).unwrap();
        store.rescan();
        let id = store.catalog[store.paths.iter().position(|p| *p == path).unwrap()].id;
        assert_eq!(store.delete_by_id(id), Ok(true));
        assert_eq!(store.delete_by_id(id), Ok(false));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
