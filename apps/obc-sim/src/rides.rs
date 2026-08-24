//! The simulator's file-backed stand-in for the flat Ride catalog.
//!
//! The **host** side of the Rides screen's catalog: stored rides live as `ride-{id}.obcr` files in
//! the tracks folder (written by [`TrackStore`](crate::track::TrackStore) at Finish). That namespace
//! is explicitly a desktop fixture convention, not a device filename. The device records and lists
//! flat objects. Synced stamps are process-local until #1398 supplies the shared flat ride-domain
//! metadata boundary.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use obc_app::{CatalogObjectId, RideSummary};
use obc_formats::io::SliceSource;
use obc_route::{ride_elevation_profile, ride_preview_polyline, Profile, RideInfo};

/// The folder-backed ride store: the catalog of ride summaries (newest first) plus, parallel to it,
/// each ride's full-width object id and desktop `ride-{id}.obcr` path.
pub struct RideStore {
    dir: PathBuf,
    catalog: Vec<RideSummary>,
    ids: Vec<CatalogObjectId>,
    paths: Vec<PathBuf>,
    synced: HashMap<CatalogObjectId, u32>,
}

impl RideStore {
    /// Open and scan the tracks folder (a missing folder scans to an empty catalog).
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        let mut s = RideStore {
            dir: dir.into(),
            catalog: Vec::new(),
            ids: Vec::new(),
            paths: Vec::new(),
            synced: HashMap::new(),
        };
        s.rescan();
        s
    }

    /// The ride catalog (summaries, newest first), for [`App::set_rides`](obc_app::App::set_rides).
    pub fn catalog(&self) -> &[RideSummary] {
        &self.catalog
    }

    /// Each catalog entry's durable object id, parallel to [`catalog`](RideStore::catalog).
    pub fn ids(&self) -> &[CatalogObjectId] {
        &self.ids
    }

    /// Re-read the folder's `ride-{id}.obcr` files into the catalog (newest first by `start_time`),
    /// each stamped with this simulator process's synced fact.
    pub fn rescan(&mut self) {
        self.catalog.clear();
        self.ids.clear();
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
        rows.sort_by_key(|r| std::cmp::Reverse(r.2.start_time)); // newest first
        for (id, path, sum) in rows {
            self.ids.push(id);
            self.paths.push(path);
            self.catalog.push(sum);
        }
    }

    /// Delete the ride with durable id `id` (the hold-to-delete, #454): remove its desktop fixture
    /// object and retire its process-local synced flag, then rescan. `true` = a file was deleted.
    /// The caller re-feeds [`App::set_rides`](obc_app::App::set_rides) so the app remaps.
    pub fn delete_by_id(&mut self, id: CatalogObjectId) -> bool {
        let Some(pos) = self.ids.iter().position(|&x| x == id) else { return false };
        let path = self.paths[pos].clone();
        if std::fs::remove_file(&path).is_err() {
            return false;
        }
        self.synced.remove(&id);
        self.rescan();
        true
    }

    /// Build the ride with durable id `id`'s recorded-track elevation [`Profile`] — the Ride
    /// detail's band fill (epic #678 T2 / #680), answering
    /// [`App::ride_track_request`](obc_app::App::ride_track_request). One read of the
    /// stored `ride-{id}.obcr` through the shared `ride_elevation_profile` (the firmware streams the
    /// same object bytes in chunks). `None` = unknown id / unreadable file — the caller parks the
    /// failure via `set_ride_profile(None)`.
    pub fn profile_by_id(&self, id: CatalogObjectId) -> Option<Profile> {
        let pos = self.ids.iter().position(|&x| x == id)?;
        let bytes = std::fs::read(&self.paths[pos]).ok()?;
        ride_elevation_profile(&SliceSource(&bytes)).ok()
    }

    /// Build the ride with durable id `id`'s decimated recorded-track shape polyline (#678
    /// rework 3), answering the preview half of the same
    /// [`App::ride_track_request`](obc_app::App::ride_track_request) drain — one more
    /// read of the stored `ride-{id}.obcr` through the shared `ride_preview_polyline` (the firmware
    /// streams the same object bytes in blocks). Empty = unknown id / unreadable file — the Ride
    /// detail's track page just leaves its slot blank.
    pub fn preview_by_id(&self, id: CatalogObjectId) -> Vec<(i32, i32)> {
        let Some(pos) = self.ids.iter().position(|&x| x == id) else { return Vec::new() };
        let Ok(bytes) = std::fs::read(&self.paths[pos]) else { return Vec::new() };
        ride_preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>(&SliceSource(&bytes))
            .map(|v| v.as_slice().to_vec())
            .unwrap_or_default()
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
    fn catalog(&self) -> &[RideSummary] {
        self.catalog()
    }
    fn ids(&self) -> &[CatalogObjectId] {
        self.ids()
    }
    fn delete_by_id(&mut self, id: CatalogObjectId) -> bool {
        self.delete_by_id(id)
    }
    fn profile_by_id(&self, id: CatalogObjectId) -> Option<Profile> {
        self.profile_by_id(id)
    }
    fn preview_by_id(&self, id: CatalogObjectId) -> Vec<(i32, i32)> {
        self.preview_by_id(id)
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
/// A filename number the band cannot hold (`N >= 2^64 - 2^32`) is therefore **not listed at all**,
/// rather than listed under an id that collides with a route: the allocator below never mints one,
/// so the only way to see this is a hand-written fixture, and an absent row is the honest answer to
/// a file whose identity this store cannot state.
fn fixture_object_id_in(p: &Path) -> Option<CatalogObjectId> {
    p.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("ride-"))
        .and_then(|n| n.strip_suffix(".obcr"))
        .and_then(|n| n.parse::<CatalogObjectId>().ok())
        .and_then(|n| n.checked_add(obc_host_core::RIDE_ID_BASE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::TrackStore;
    use obc_app::TrackAction;
    use obc_ports::TrackPoint;
    use obc_route::RideStats;

    /// Record and save one short ride into `dir` (session `id`), producing a real v3 desktop object
    /// with elevation + geometry — the folder ride store's conformance fixture, built through the
    /// same public `TrackStore` path the app drives.
    fn record_ride(dir: &Path, session: u32, name: &str) {
        let mut ts = TrackStore::open(dir);
        ts.reconcile(None, Some(session), Some(name), None);
        if let Some(sink) = ts.sink() {
            for k in 0..6u32 {
                sink.record(TrackPoint {
                    lon: 8_000_000 + k as i32 * 200,
                    lat: 46_000_000 + k as i32 * 200,
                    ele: 1000 + k as i16 * 10,
                    t_ms: k * 1000,
                    segment_start: k == 0,
                    hr: None,
                    cadence: None,
                    power: None,
                })
                .unwrap();
            }
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
        ts.reconcile(Some(TrackAction::Save), None, Some(name), Some(stats));
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
}
