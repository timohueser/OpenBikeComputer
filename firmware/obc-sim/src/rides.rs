//! The simulator's ride store — a stand-in for the device's `/tracks` folder (issue #454).
//!
//! The **host** side of the Rides screen's catalog: stored rides live as `RD{id}.ORD` files in the
//! tracks folder (written by [`TrackStore`](crate::track::TrackStore) at Finish, exactly as the
//! device does), and the phone's "downloaded at least once" flags live in the same folder's
//! `SYNCED.SET` sidecar. This scans both into a [`RideSummary`] catalog for
//! [`App::set_rides`](obc_app::App::set_rides), and deletes a ride (file + sidecar flag) for the
//! hold-to-delete footer. The firmware provides the identical surface over FatFs.

use std::path::{Path, PathBuf};

use obc_app::{decode_synced_rides, encode_synced_rides, RideSummary, SyncedRides, SYNCED_RIDES_MAX_LEN};
use obc_formats::io::SliceSource;
use obc_route::{ride_elevation_profile, ride_preview_polyline, Profile, RideInfo};

/// The synced-ride sidecar filename in the tracks folder — matches the device's `SYNCED_SET`.
const SYNCED_SET: &str = "SYNCED.SET";

/// The folder-backed ride store: the catalog of ride summaries (newest first) plus, parallel to it,
/// each ride's durable object id and `RD{id}.ORD` path.
pub struct RideStore {
    dir: PathBuf,
    catalog: Vec<RideSummary>,
    ids: Vec<u16>,
    paths: Vec<PathBuf>,
}

impl RideStore {
    /// Open and scan the tracks folder (a missing folder scans to an empty catalog).
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        let mut s = RideStore { dir: dir.into(), catalog: Vec::new(), ids: Vec::new(), paths: Vec::new() };
        s.rescan();
        s
    }

    /// The ride catalog (summaries, newest first), for [`App::set_rides`](obc_app::App::set_rides).
    pub fn catalog(&self) -> &[RideSummary] {
        &self.catalog
    }

    /// Each catalog entry's durable object id, parallel to [`catalog`](RideStore::catalog).
    pub fn ids(&self) -> &[u16] {
        &self.ids
    }

    /// Re-read the folder's `RD{id}.ORD` files into the catalog (newest first by `start_time`), each
    /// stamped with its synced flag from the `SYNCED.SET` sidecar. A torn/missing sidecar reads as
    /// "nothing synced" (the codec's contract).
    pub fn rescan(&mut self) {
        self.catalog.clear();
        self.ids.clear();
        self.paths.clear();
        let synced = self.load_synced();
        let mut rows: Vec<(u16, PathBuf, RideSummary)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                let p = e.path();
                let Some(id) = ride_id_in(&p) else { continue };
                if let Ok(bytes) = std::fs::read(&p) {
                    if let Ok(info) = RideInfo::read(&SliceSource(&bytes)) {
                        rows.push((id, p, RideSummary::from_info(&info, synced.contains(id))));
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

    /// Delete the ride with durable id `id` (the hold-to-delete, #454): remove its `RD{id}.ORD` and
    /// retire its synced flag, then rescan. `true` = a file was deleted. The caller re-feeds
    /// [`App::set_rides`](obc_app::App::set_rides) so the app remaps.
    pub fn delete_by_id(&mut self, id: u16) -> bool {
        let Some(pos) = self.ids.iter().position(|&x| x == id) else { return false };
        let path = self.paths[pos].clone();
        if std::fs::remove_file(&path).is_err() {
            return false;
        }
        let mut set = self.load_synced();
        if set.remove(id) {
            self.save_synced(&set);
        }
        self.rescan();
        true
    }

    /// Build the ride with durable id `id`'s recorded-track elevation [`Profile`] — the Ride
    /// detail's band fill (epic #678 T2 / #680), answering
    /// [`App::ride_track_request`](obc_app::App::ride_track_request). One read of the
    /// stored `RD{id}.ORD` through the shared `ride_elevation_profile` (the firmware streams the
    /// same bytes off SD in chunks). `None` = unknown id / unreadable file — the caller parks the
    /// failure via `set_ride_profile(None)`.
    pub fn profile_by_id(&self, id: u16) -> Option<Profile> {
        let pos = self.ids.iter().position(|&x| x == id)?;
        let bytes = std::fs::read(&self.paths[pos]).ok()?;
        ride_elevation_profile(&SliceSource(&bytes)).ok()
    }

    /// Build the ride with durable id `id`'s decimated recorded-track shape polyline (#678
    /// rework 3), answering the preview half of the same
    /// [`App::ride_track_request`](obc_app::App::ride_track_request) drain — one more
    /// read of the stored `RD{id}.ORD` through the shared `ride_preview_polyline` (the firmware
    /// streams the same bytes off SD in blocks). Empty = unknown id / unreadable file — the Ride
    /// detail's track page just leaves its slot blank.
    pub fn preview_by_id(&self, id: u16) -> Vec<(i32, i32)> {
        let Some(pos) = self.ids.iter().position(|&x| x == id) else { return Vec::new() };
        let Ok(bytes) = std::fs::read(&self.paths[pos]) else { return Vec::new() };
        ride_preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>(&SliceSource(&bytes))
            .map(|v| v.as_slice().to_vec())
            .unwrap_or_default()
    }

    /// Read the synced-ride sidecar into a [`SyncedRides`] set (empty on a missing/torn file).
    fn load_synced(&self) -> SyncedRides {
        match std::fs::read(self.dir.join(SYNCED_SET)) {
            Ok(bytes) => decode_synced_rides(&bytes),
            Err(_) => SyncedRides::new(),
        }
    }

    /// Persist the synced-ride sidecar (creating the folder if needed).
    fn save_synced(&self, set: &SyncedRides) {
        let mut buf = [0u8; SYNCED_RIDES_MAX_LEN];
        let n = encode_synced_rides(set, &mut buf);
        let _ = std::fs::create_dir_all(&self.dir);
        let _ = std::fs::write(self.dir.join(SYNCED_SET), &buf[..n]);
    }
}

/// The shared dispatcher ([`obc_host_core::HostLoop`]) drives the ride catalog + per-ride track
/// reads through this trait — the same delete/re-feed/track-fill sequencing the board runs.
impl obc_host_core::RideRepository for RideStore {
    fn catalog(&self) -> &[RideSummary] {
        self.catalog()
    }
    fn ids(&self) -> &[u16] {
        self.ids()
    }
    fn delete_by_id(&mut self, id: u16) -> bool {
        self.delete_by_id(id)
    }
    fn profile_by_id(&self, id: u16) -> Option<Profile> {
        self.profile_by_id(id)
    }
    fn preview_by_id(&self, id: u16) -> Vec<(i32, i32)> {
        self.preview_by_id(id)
    }
    /// A `Save` just wrote a fresh `RD{id}.ORD`; re-scan so it appears in the Rides menu live.
    fn refresh(&mut self) {
        self.rescan();
    }
}

/// The durable object id in an `RD{id}.ORD` path, or `None` for any other file.
fn ride_id_in(p: &Path) -> Option<u16> {
    p.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("RD"))
        .and_then(|n| n.strip_suffix(".ORD"))
        .and_then(|n| n.parse::<u16>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::TrackStore;
    use obc_app::TrackAction;
    use obc_ports::TrackPoint;
    use obc_route::RideStats;

    /// Record and save one short ride into `dir` (session `id`), producing a real `RD{id}.ORD` with
    /// elevation + geometry — the folder ride store's conformance fixture, built through the same
    /// public `TrackStore` path the app drives.
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
        let dir = std::env::temp_dir().join(format!("obc-ride-conf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        record_ride(&dir, 1, "Ride One");
        record_ride(&dir, 2, "Ride Two");

        let mut store = RideStore::open(&dir);
        assert_eq!(store.catalog().len(), 2, "two saved rides scanned");
        obc_host_core::conformance::ride_repository_suite(&mut store, true);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
