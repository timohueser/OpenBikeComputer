//! The simulator's trip store — a stand-in for the device's `TP{id}.OBT` trip files.
//!
//! The **host** side of the [`obc_route::TripMeta`] codec: trips live as `.obt` files beside the
//! `.obcr` routes (the device stores `TP{id}.OBT` beside `RT{id}.OBR`). Scans the folder into a list
//! of trips, each with a session id, and hands them to [`App::set_trips`](obc_app::App::set_trips)
//! as [`TripInput`](obc_app::TripInput)s — the app resolves the stage route ids against the route
//! catalog. Deleting a trip removes its backing `.obt` (the protocol-level trip delete is
//! non-cascading; a "delete trip *and* its routes" is the initiating UI's composition — TR3).

use std::path::{Path, PathBuf};

use obc_app::TripInput;
use obc_route::{SliceSource, TripMeta};

/// The scanned trips folder: each trip's session id + its decoded [`TripMeta`], parallel to its file
/// path. Trip ids live in their own namespace (separate from routes/rides, spec §4.1).
pub struct TripStore {
    dir: PathBuf,
    trips: Vec<TripMeta>,
    ids: Vec<u16>,
    paths: Vec<PathBuf>,
    /// The session id registry (path → id) + the next fresh one, append-only so a file keeps its id
    /// across rescans — the sim's face of the device's `TP{id}` counter, used only for `.obt` files
    /// whose name doesn't already encode an id.
    assigned: Vec<(PathBuf, u16)>,
    next_id: u16,
}

impl TripStore {
    /// Open and scan the trips folder (the same folder as the routes — trips sit beside routes). A
    /// missing folder scans to an empty list.
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        let mut s = TripStore {
            dir: dir.into(),
            trips: Vec::new(),
            ids: Vec::new(),
            paths: Vec::new(),
            assigned: Vec::new(),
            next_id: 0,
        };
        s.rescan();
        s
    }

    /// The scanned trips as [`TripInput`]s for [`App::set_trips`](obc_app::App::set_trips) — id, name,
    /// and stage route ids borrowed from the resident [`TripMeta`]s. Call **after**
    /// `set_routes_with_ids` so the stage ids resolve against the route catalog.
    pub fn inputs(&self) -> Vec<TripInput<'_>> {
        self.trips
            .iter()
            .zip(&self.ids)
            .map(|(t, &id)| TripInput { id, name: t.name.as_str(), stage_ids: t.stage_ids.as_slice() })
            .collect()
    }

    /// Re-read the folder's `.obt` files (sorted by filename), decoding each with the production
    /// [`TripMeta`] codec. A file that fails to decode (wrong version / torn) is skipped. Over the
    /// resident cap, the store warns and keeps the first [`MAX_TRIPS`](obc_app::MAX_TRIPS) — mirroring
    /// the route-scan overflow (the app's `set_trips` also truncates).
    pub fn rescan(&mut self) {
        self.trips.clear();
        self.ids.clear();
        self.paths.clear();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            let mut files: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                // Case-insensitive so the device's uppercase FAT name (`TP{id}.OBT`) and a
                // side-loaded lowercase `.obt` both scan.
                .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("obt")))
                .collect();
            files.sort();
            for p in files {
                if self.trips.len() >= obc_app::MAX_TRIPS {
                    eprintln!(
                        "trip scan: more than {} trips — listing the first {}",
                        obc_app::MAX_TRIPS,
                        obc_app::MAX_TRIPS
                    );
                    break;
                }
                if let Ok(bytes) = std::fs::read(&p) {
                    if let Ok(meta) = TripMeta::read(&SliceSource(&bytes)) {
                        let id = self.id_for(&p);
                        self.trips.push(meta);
                        self.ids.push(id);
                        self.paths.push(p);
                    }
                }
            }
        }
    }

    /// The session id for `path`: from a `TP{id}.OBT` filename when present (the device's own naming),
    /// else the registered / freshly-assigned fake id. Append-only for the session — ids are never
    /// reused, matching the device's contract.
    fn id_for(&mut self, path: &Path) -> u16 {
        if let Some(id) = id_from_filename(path) {
            return id;
        }
        if let Some((_, id)) = self.assigned.iter().find(|(p, _)| p == path) {
            return *id;
        }
        let id = self.next_id;
        self.assigned.push((path.to_path_buf(), id));
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    /// Delete the trip with session id `id` (the on-device trip delete): remove its `.obt` from the
    /// folder and rescan. `true` = a file was deleted. Non-cascading — member routes are untouched
    /// (spec §7.7); the caller then re-feeds [`App::set_trips`](obc_app::App::set_trips).
    pub fn delete_by_id(&mut self, id: u16) -> bool {
        let Some(pos) = self.ids.iter().position(|&x| x == id) else { return false };
        let path = self.paths[pos].clone();
        if std::fs::remove_file(&path).is_err() {
            return false;
        }
        self.rescan();
        true
    }
}

/// Parse a `TP{id}.OBT` filename into its trip id (case-insensitive `.obt`), else `None` — a trip
/// file the app named differently (or a side-loaded one) gets a fake session id instead.
fn id_from_filename(path: &Path) -> Option<u16> {
    let stem = path.file_stem()?.to_str()?;
    let digits = stem.strip_prefix("TP").or_else(|| stem.strip_prefix("tp"))?;
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::RouteStore;
    use obc_app::{App, AppState};
    use obc_route::{write_trip, RouteSummary, SliceSource};

    /// The committed sample route (`assets/grimsel-climb.obcr`) — the "Grimsel Climb" OBCR the fixture
    /// trip groups copies of.
    const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/grimsel-climb.obcr");
    /// The committed fixture trip (`assets/TP1.OBT`): "Alpen Traverse", stage ids `[0, 1, 99]` —
    /// the first two are the sorted-scan ids of any two staged routes, 99 the deliberate dangling
    /// ref. Copy it into a `--routes-dir` beside two or more routes for a groupable menu; TR3's
    /// snapshot harness stages exactly this file.
    const TRIP_ASSET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/TP1.OBT");

    /// Stage a routes folder with three copies of the sample route (`a`/`b`/`c`, so their sorted-scan
    /// ids are 0/1/2) plus the committed `TP1.OBT` trip grouping the first two (ids 0, 1) with one
    /// dangling ref (99). Route `c` (id 2) stays loose — a groupable menu: one folder + one
    /// top-level route.
    fn stage_fixture(tag: &str) -> std::path::PathBuf {
        let route = std::fs::read(SAMPLE).expect("sample route asset readable");
        let dir = std::env::temp_dir().join(format!("obc-trips-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["a.obcr", "b.obcr", "c.obcr"] {
            std::fs::write(dir.join(name), &route).unwrap();
        }
        std::fs::copy(TRIP_ASSET, dir.join("TP1.OBT")).expect("trip asset readable");
        dir
    }

    /// The committed `TP1.OBT` asset equals the production writer's output byte-for-byte — the
    /// asset's provenance pin (regenerate by re-running `write_trip` with these arguments).
    #[test]
    fn committed_trip_asset_matches_the_production_writer() {
        struct VecSink(Vec<u8>);
        impl obc_route::ByteSink for VecSink {
            fn write(&mut self, b: &[u8]) -> Result<(), obc_route::Error> {
                self.0.extend_from_slice(b);
                Ok(())
            }
            fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), obc_route::Error> {
                let o = off as usize;
                self.0[o..o + b.len()].copy_from_slice(b);
                Ok(())
            }
        }
        let mut sink = VecSink(Vec::new());
        write_trip("Alpen Traverse", &[0, 1, 99], &mut sink).unwrap();
        assert_eq!(std::fs::read(TRIP_ASSET).expect("trip asset readable"), sink.0);
    }

    fn sample_distance_km() -> u32 {
        let bytes = std::fs::read(SAMPLE).unwrap();
        RouteSummary::read(&SliceSource(&bytes)).unwrap().distance_km
    }

    /// The sim scans `.obt` beside `.obcr`, ids the trip from its `TP{id}.OBT` name, and the app
    /// groups the two referenced routes into a folder — leaving the loose route top-level and the
    /// dangling ref dropped.
    #[test]
    fn scans_obt_and_groups_into_a_folder() {
        let dir = stage_fixture("group");
        let route_store = RouteStore::open(&dir);
        let trip_store = TripStore::open(&dir);

        assert_eq!(route_store.catalog().len(), 3);
        assert_eq!(route_store.ids(), &[0, 1, 2]); // sorted a/b/c → fake scan-order ids

        let inputs = trip_store.inputs();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].id, 1); // from the TP1.OBT filename
        assert_eq!(inputs[0].name, "Alpen Traverse");
        assert_eq!(inputs[0].stage_ids, &[0, 1, 99]);

        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        app.set_routes_with_ids(route_store.catalog(), route_store.ids());
        app.set_trips(&inputs);

        assert_eq!(app.trips().len(), 1);
        let t = &app.trips()[0];
        assert_eq!(t.stage_indices.as_slice(), &[0, 1]); // routes 0 & 1 filed; 99 dropped
        assert_eq!(t.distance_km, 2 * sample_distance_km()); // summed over the two resolvable stages
        assert!(app.route_filed(0));
        assert!(app.route_filed(1));
        assert!(!app.route_filed(2)); // the loose route stays top-level

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Deleting a trip removes its backing `.obt` (non-cascading — the routes stay) and the rescanned
    /// store re-feeds an empty trip list.
    #[test]
    fn delete_removes_the_backing_obt_only() {
        let dir = stage_fixture("delete");
        let mut trip_store = TripStore::open(&dir);
        assert_eq!(trip_store.inputs().len(), 1);

        assert!(trip_store.delete_by_id(1));
        assert!(!dir.join("TP1.OBT").exists(), "the trip file is gone");
        // The member route files are untouched (non-cascading, spec §7.7).
        assert!(dir.join("a.obcr").exists());
        assert!(dir.join("b.obcr").exists());
        assert!(dir.join("c.obcr").exists());

        assert!(trip_store.inputs().is_empty());
        // A second delete of the retired id is a no-op.
        assert!(!trip_store.delete_by_id(1));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
