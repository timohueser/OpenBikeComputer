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

    /// Make the active route match `want`, (re)reading its bytes from disk only on a change.
    /// Cheap to call every frame.
    pub fn sync_active(&mut self, want: Option<usize>) {
        if want == self.active && (want.is_none() == self.active_bytes.is_none()) {
            return;
        }
        self.active = want;
        self.active_bytes = want.and_then(|i| self.paths.get(i)).and_then(|p| std::fs::read(p).ok());
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

    /// GPX import (USB-drop equivalent) isn't wired up on the web yet — a file-input
    /// upload path replaces the native dialog later.
    pub fn import_gpx(&mut self, _gpx_path: &Path) -> Result<RouteStats, String> {
        Err("GPX import is not available in the web build yet".into())
    }

    pub fn sync_active(&mut self, want: Option<usize>) {
        self.active = want.filter(|&i| i < self.bytes.len());
    }

    pub fn active_source(&self) -> Option<SliceSource<'_>> {
        self.active.and_then(|i| self.bytes.get(i)).map(|b| SliceSource(b.as_slice()))
    }
}
