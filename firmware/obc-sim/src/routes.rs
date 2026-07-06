//! The simulator's route store — a stand-in for the device's SD card.
//!
//! The **host** side of the [`obc_route`] abstraction: routes live in a folder instead of
//! `.obcr` files on an SD card. Backs [`ByteSource`](obc_route::ByteSource) with `std` file
//! I/O, scans the folder into a [`RouteSummary`] catalog, converts imported GPX, and serves
//! the active route's bytes to stream. The firmware provides the same surface over FatFs;
//! nothing above this (`obc-app`, `obc-render`) knows it's a folder.

use std::path::{Path, PathBuf};

#[cfg(not(target_arch = "wasm32"))]
use obc_route::gpx_to_obcr;
use obc_route::{RouteStats, RouteSummary, SliceSource};

#[cfg(not(target_arch = "wasm32"))]
use crate::vec_sink::VecSink;

/// The reserved computed-route file the on-device router's output lands in (epic #116, R4):
/// auto-overwritten on every plan, scanned into the catalog like any other route. The device's
/// FatFs twin is `/routes/_NAV.OBR` (embedded-sdmmc can't write the 4-char LFN extension).
#[cfg(not(target_arch = "wasm32"))]
const NAV_ROUTE_FILE: &str = "_nav.obcr";

/// The folder-backed route store: the catalog of summaries plus the bytes of the one active route.
#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
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
    /// file's session id, ready for `App::notify_route_uploaded(id, false)`.
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
    /// `App::notify_route_uploaded(id, true)`.
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

    /// Make the active route match `want`, (re)reading its bytes from disk only on a change.
    /// Cheap to call every frame.
    pub fn sync_active(&mut self, want: Option<usize>) {
        if want == self.active && (want.is_none() == self.active_bytes.is_none()) {
            return;
        }
        self.active = want;
        self.active_bytes = want.and_then(|i| self.paths.get(i)).and_then(|p| std::fs::read(p).ok());
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

// --- Web (wasm32) route store ---------------------------------------------------
//
// No folder to scan, so the web build keeps the catalog + route bytes in memory. Same
// public surface as the native store, so `gui.rs` drives both identically.
#[cfg(target_arch = "wasm32")]
pub struct RouteStore {
    catalog: Vec<RouteSummary>,
    ids: Vec<u16>,
    bytes: Vec<Vec<u8>>,
    active: Option<usize>,
}

/// The web store's reserved id for the in-memory nav route (out of the small positional band the
/// embedded catalog uses), so a re-plan replaces the previous computed route in place.
#[cfg(target_arch = "wasm32")]
const WASM_NAV_ID: u16 = u16::MAX;

#[cfg(target_arch = "wasm32")]
impl RouteStore {
    /// `dir` is ignored on the web; the signature matches the native store.
    pub fn open(_dir: impl Into<PathBuf>) -> Self {
        let mut s = RouteStore { catalog: Vec::new(), ids: Vec::new(), bytes: Vec::new(), active: None };
        s.seed_embedded();
        s
    }

    /// Load the demo routes compiled into the wasm binary. Add `include_bytes!` entries to
    /// grow the menu.
    fn seed_embedded(&mut self) {
        for route in [include_bytes!("../assets/grimsel-climb.obcr").as_slice()] {
            if let Ok(sum) = RouteSummary::read(&SliceSource(route)) {
                self.catalog.push(sum);
                self.ids.push(self.ids.len() as u16); // fixed catalog → positional ids are stable
                self.bytes.push(route.to_vec());
            }
        }
    }

    pub fn catalog(&self) -> &[RouteSummary] {
        &self.catalog
    }

    /// Each catalog entry's id, parallel to [`catalog`](RouteStore::catalog) (fixed on the web).
    pub fn ids(&self) -> &[u16] {
        &self.ids
    }

    /// No folder to re-read on the web; the embedded catalog is fixed.
    pub fn rescan(&mut self) {}

    /// Upload injection is a native control-panel tool; the fixed web catalog has no store to move.
    pub fn duplicate_route(&mut self, _i: usize) -> Option<u16> {
        None
    }

    /// See [`duplicate_route`](RouteStore::duplicate_route) — unavailable on the web build.
    pub fn touch_route(&mut self, _i: usize) -> Option<u16> {
        None
    }

    /// GPX import (USB-drop equivalent) isn't wired up on the web yet — a file-input
    /// upload path replaces the native dialog later.
    pub fn import_gpx(&mut self, _gpx_path: &Path) -> Result<RouteStats, String> {
        Err("GPX import is not available in the web build yet".into())
    }

    /// The in-memory twin of the native store's reserved `_nav.obcr` write: replace (or append)
    /// the computed route under the fixed [`WASM_NAV_ID`] and return it.
    pub fn write_nav_route(&mut self, bytes: &[u8]) -> Option<u16> {
        let sum = RouteSummary::read(&SliceSource(bytes)).ok()?;
        match self.ids.iter().position(|&id| id == WASM_NAV_ID) {
            Some(pos) => {
                self.catalog[pos] = sum;
                self.bytes[pos] = bytes.to_vec();
            }
            None => {
                self.catalog.push(sum);
                self.ids.push(WASM_NAV_ID);
                self.bytes.push(bytes.to_vec());
            }
        }
        Some(WASM_NAV_ID)
    }

    pub fn sync_active(&mut self, want: Option<usize>) {
        self.active = want.filter(|&i| i < self.bytes.len());
    }

    /// See the native twin: force a re-read after the nav bytes are replaced under an unchanged
    /// index (the web store serves bytes by index, so only the reset matters).
    pub fn invalidate_active(&mut self) {
        self.active = None;
    }

    /// Delete the route with id `id` from the in-memory catalog (the web build's face of the
    /// on-device hold-to-delete, epic #447 P6). `true` = removed. The id isn't re-issued (the
    /// embedded catalog is fixed and positional).
    pub fn delete_by_id(&mut self, id: u16) -> bool {
        let Some(pos) = self.ids.iter().position(|&x| x == id) else { return false };
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
        true
    }

    pub fn active_source(&self) -> Option<SliceSource<'_>> {
        self.active.and_then(|i| self.bytes.get(i)).map(|b| SliceSource(b.as_slice()))
    }
}
