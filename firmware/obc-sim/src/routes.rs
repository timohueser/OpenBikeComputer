//! The simulator's route store — a stand-in for the device's SD card.
//!
//! The **host** side of the [`obc_route`] abstraction: routes live in a folder instead of
//! `.obcr` files on an SD card. Backs [`ByteSource`](obc_route::ByteSource) with `std` file
//! I/O, scans the folder into a [`RouteSummary`] catalog, converts imported GPX, and serves
//! the active route's bytes to stream. The firmware provides the same surface over FatFs;
//! nothing above this (`obc-app`, `obc-render`) knows it's a folder.

use std::path::{Path, PathBuf};

use obc_route::gpx_to_obcr;
use obc_route::{RouteStats, RouteSummary, SliceSource};

use obc_host_core::VecSink;

/// The reserved computed-route file the on-device router's output lands in (epic #116, R4):
/// auto-overwritten on every plan, scanned into the catalog like any other route. The device's
/// FatFs twin is `/routes/_NAV.OBR` (embedded-sdmmc can't write the 4-char LFN extension).
const NAV_ROUTE_FILE: &str = "_nav.obcr";

/// The folder-backed route store: the catalog of summaries plus the bytes of the one active route.
pub struct RouteStore {
    dir: PathBuf,
    catalog: Vec<RouteSummary>,
    paths: Vec<PathBuf>,
    /// Each catalog entry's **session-stable id**, parallel to `catalog`/`paths` — the sim's face
    /// of the device's durable object ids (#450). Handed to
    /// [`App::set_routes_with_ids`](obc_app::App::set_routes_with_ids) so a mid-session rescan
    /// (a dropped-in `.obcr`, a GPX import, the panel's store-changed button) exercises the same
    /// identity remap the firmware relies on.
    ids: Vec<u16>,
    /// The session id registry (path → id) + the next fresh one. Append-only, so a file keeps its
    /// id across rescans no matter what is added or removed around it — the device's `RT{id}` /
    /// side-load-registry behaviour in miniature.
    assigned: Vec<(PathBuf, u16)>,
    next_id: u16,
    active: Option<usize>,
    active_bytes: Option<Vec<u8>>,
}

impl RouteStore {
    /// Open and scan the routes folder. A missing folder is fine (scans to an empty catalog);
    /// it's created lazily on the first import.
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        let mut s = RouteStore {
            dir: dir.into(),
            catalog: Vec::new(),
            paths: Vec::new(),
            ids: Vec::new(),
            assigned: Vec::new(),
            next_id: 0,
            active: None,
            active_bytes: None,
        };
        s.rescan();
        s
    }

    /// The route catalog (summaries), for [`App::set_routes_with_ids`](obc_app::App::set_routes_with_ids).
    pub fn catalog(&self) -> &[RouteSummary] {
        &self.catalog
    }

    /// Each catalog entry's session-stable id, parallel to [`catalog`](RouteStore::catalog).
    pub fn ids(&self) -> &[u16] {
        &self.ids
    }

    /// Re-read the folder's `.obcr` files into the catalog (sorted by filename), each keeping its
    /// session-stable id from the registry (fresh files get the next one).
    pub fn rescan(&mut self) {
        self.catalog.clear();
        self.paths.clear();
        self.ids.clear();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            let mut files: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("obcr"))
                .collect();
            files.sort();
            for p in files {
                if let Ok(bytes) = std::fs::read(&p) {
                    if let Ok(sum) = RouteSummary::read(&SliceSource(&bytes)) {
                        let id = self.id_for(&p);
                        self.catalog.push(sum);
                        self.paths.push(p);
                        self.ids.push(id);
                    }
                }
            }
        }
        // A reshuffled folder can invalidate the active index; drop it if so. (The *app*'s
        // active_route is remapped by id in `set_routes_with_ids`; `sync_active` then re-feeds
        // this store the remapped index.)
        if self.active.is_some_and(|i| i >= self.catalog.len()) {
            self.active = None;
            self.active_bytes = None;
        }
    }

    /// The session id for `path`: registered, or freshly assigned. Append-only for the session —
    /// ids are never reused, matching the device's contract.
    fn id_for(&mut self, path: &Path) -> u16 {
        if let Some((_, id)) = self.assigned.iter().find(|(p, _)| p == path) {
            return *id;
        }
        let id = self.next_id;
        self.assigned.push((path.to_path_buf(), id));
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    /// Convert a GPX file into the store (named after its file stem) and rescan — the same
    /// conversion the device runs on a USB drop.
    pub fn import_gpx(&mut self, gpx_path: &Path) -> Result<RouteStats, String> {
        let gpx = std::fs::read(gpx_path).map_err(|e| format!("read {}: {e}", gpx_path.display()))?;
        let stem = gpx_path.file_stem().and_then(|s| s.to_str()).unwrap_or("route");
        let mut sink = VecSink::default();
        let stats = gpx_to_obcr(&SliceSource(&gpx), stem, &mut sink)
            .map_err(|e| format!("convert {}: {e:?}", gpx_path.display()))?;
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("create {}: {e}", self.dir.display()))?;
        let out = self.dir.join(format!("{stem}.obcr"));
        std::fs::write(&out, sink.bytes()).map_err(|e| format!("write {}: {e}", out.display()))?;
        self.rescan();
        Ok(stats)
    }

    /// Delete the route with session id `id` (the on-device hold-to-delete, epic #447 P6): remove its
    /// `.obcr` from the folder and rescan. `true` = a file was deleted. The registry is append-only,
    /// so the id is retired, not reused — mirroring the device's never-reuse contract. The caller then
    /// re-feeds [`App::set_routes_with_ids`](obc_app::App::set_routes_with_ids) so the app remaps.
    pub fn delete_by_id(&mut self, id: u16) -> bool {
        let Some(pos) = self.ids.iter().position(|&x| x == id) else { return false };
        let path = self.paths[pos].clone();
        if std::fs::remove_file(&path).is_err() {
            return false;
        }
        self.rescan();
        true
    }

    /// Duplicate route `i`'s file under a fresh name — the control panel's "**new** upload"
    /// stand-in (a real upload writes a new file to `/routes`) — then rescan and return the new
    /// file's session id, ready for `App::apply_event(id, false)`.
    pub fn duplicate_route(&mut self, i: usize) -> Option<u16> {
        let src = self.paths.get(i)?.clone();
        let bytes = std::fs::read(&src).ok()?;
        let stem = src.file_stem()?.to_str()?.to_string();
        let out = (1..1000).map(|n| self.dir.join(format!("{stem}-up{n}.obcr"))).find(|c| !c.exists())?;
        std::fs::write(&out, &bytes).ok()?;
        self.rescan();
        let k = self.paths.iter().position(|p| p == &out)?;
        Some(self.ids[k])
    }

    /// Rewrite route `i`'s file in place with its own bytes — the control panel's
    /// "**replace-by-id** upload" stand-in (same id, the bytes on disk swapped under any open
    /// handle, as the device's replace-commit does). Returns the route's (unchanged) id for
    /// `App::apply_event(id, true)`.
    pub fn touch_route(&mut self, i: usize) -> Option<u16> {
        let path = self.paths.get(i)?.clone();
        let bytes = std::fs::read(&path).ok()?;
        std::fs::write(&path, bytes).ok()?;
        self.ids.get(i).copied()
    }

    /// Write the router's emitted OBCR to the reserved nav-route file (epic #116, R4) —
    /// `_nav.obcr`, overwritten in place so consecutive plans never accumulate — then rescan and
    /// return the file's session-stable id (stable across overwrites: the id registry keys on the
    /// path). `None` on an I/O failure — the caller degrades to the generic routing-failure tier.
    pub fn write_nav_route(&mut self, bytes: &[u8]) -> Option<u16> {
        std::fs::create_dir_all(&self.dir).ok()?;
        let out = self.dir.join(NAV_ROUTE_FILE);
        std::fs::write(&out, bytes).ok()?;
        self.rescan();
        let k = self.paths.iter().position(|p| p == &out)?;
        Some(self.ids[k])
    }

    /// The mini elevation sparkline for the route with session id `id` (#682): read its `.obcr`
    /// bytes and stream them once through [`obc_route::elevation_sparkline`] — the host side of the
    /// route-upload seam, mirroring the board's build-at-commit-time. `None` when the id is unknown,
    /// the read fails, or the route carries no elevation.
    pub fn elevation_sparkline(&self, id: u16) -> Option<[u8; obc_route::SPARKLINE_BUCKETS]> {
        let pos = self.ids.iter().position(|&x| x == id)?;
        let bytes = std::fs::read(&self.paths[pos]).ok()?;
        obc_route::elevation_sparkline(&SliceSource(&bytes))
    }

    /// Make the active route match `want`, (re)reading its bytes from disk only on a change.
    /// Returns whether the active bytes were (re)read this call — the reparse signal the resident
    /// [`ActiveRouteSession`](obc_host_core::ActiveRouteSession) gates on. Cheap to call every frame.
    pub fn sync_active(&mut self, want: Option<usize>) -> bool {
        if want == self.active && (want.is_none() == self.active_bytes.is_none()) {
            return false;
        }
        self.active = want;
        self.active_bytes = want.and_then(|i| self.paths.get(i)).and_then(|p| std::fs::read(p).ok());
        true
    }

    /// Force the active route's bytes to re-read from disk on the next [`sync_active`] even if the
    /// index is unchanged — the nav flow overwrites `_nav.obcr` **under** an unchanged catalog
    /// index on a re-route, which the change-gated `sync_active` would otherwise keep serving
    /// stale.
    pub fn invalidate_active(&mut self) {
        self.active = None;
        self.active_bytes = None;
    }

    /// A [`ByteSource`](obc_route::ByteSource) over the active route's bytes, for
    /// opening a [`RouteReader`](obc_route::RouteReader) to stream geometry from.
    pub fn active_source(&self) -> Option<SliceSource<'_>> {
        self.active_bytes.as_deref().map(SliceSource)
    }
}

/// The shared dispatcher ([`obc_host_core::HostLoop`]) and nav-commit path drive the folder store
/// through this trait, so the exact delete → rescan → re-feed and write → rescan → invalidate orders
/// live in one place for every host.
impl obc_host_core::RouteRepository for RouteStore {
    fn catalog(&self) -> &[RouteSummary] {
        self.catalog()
    }
    fn ids(&self) -> &[u16] {
        self.ids()
    }
    fn delete_by_id(&mut self, id: u16) -> bool {
        self.delete_by_id(id)
    }
    fn write_nav_route(&mut self, bytes: &[u8]) -> Option<u16> {
        self.write_nav_route(bytes)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed sample OBCR the folder store seeds from (twice, for a ≥2-route catalog).
    const ROUTE: &[u8] = include_bytes!("../assets/grimsel-climb.obcr");

    fn temp_route_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("obc-route-conf-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.obcr"), ROUTE).unwrap();
        std::fs::write(dir.join("b.obcr"), ROUTE).unwrap();
        dir
    }

    /// The **folder-backed** route store passes the same `obc-host-core` conformance suite the
    /// in-memory store does — active-replacement/reparse signal, delete + id retirement, and the
    /// reserved nav commit.
    #[test]
    fn folder_route_store_passes_the_conformance_suite() {
        let dir = temp_route_dir("suite");
        let mut store = RouteStore::open(&dir);
        obc_host_core::conformance::route_repository_suite(&mut store, ROUTE);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// …and the app-driven identity remap (the active route stays on its durable id across a delete).
    #[test]
    fn folder_route_store_passes_identity_remap() {
        let dir = temp_route_dir("remap");
        let mut store = RouteStore::open(&dir);
        obc_host_core::conformance::route_identity_remap(&mut store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
