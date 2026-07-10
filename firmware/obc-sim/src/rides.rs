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
use obc_route::{ride_elevation_profile, Profile, RideInfo, SliceSource};

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
    /// [`App::take_ride_track_request`](obc_app::App::take_ride_track_request). One read of the
    /// stored `RD{id}.ORD` through the shared `ride_elevation_profile` (the firmware streams the
    /// same bytes off SD in chunks). `None` = unknown id / unreadable file — the caller parks the
    /// failure via `set_ride_profile(None)`.
    pub fn profile_by_id(&self, id: u16) -> Option<Profile> {
        let pos = self.ids.iter().position(|&x| x == id)?;
        let bytes = std::fs::read(&self.paths[pos]).ok()?;
        ride_elevation_profile(&SliceSource(&bytes)).ok()
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

/// The durable object id in an `RD{id}.ORD` path, or `None` for any other file.
fn ride_id_in(p: &Path) -> Option<u16> {
    p.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("RD"))
        .and_then(|n| n.strip_suffix(".ORD"))
        .and_then(|n| n.parse::<u16>().ok())
}
