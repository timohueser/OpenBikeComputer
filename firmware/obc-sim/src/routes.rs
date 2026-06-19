//! The simulator's route store — a stand-in for the device's SD card.
//!
//! On the device routes live as `.obcr` files on an SD card; here they live in a
//! folder. This module is the **host** side of the [`obc_route`] abstraction: it
//! backs [`ByteSource`](obc_route::ByteSource)/[`ByteSink`](obc_route::ByteSink) with
//! `std` file I/O, scans the folder into a [`RouteSummary`] catalog for the Route menu,
//! converts a dropped/`--import`ed GPX into the folder, and serves the active route's
//! bytes for the renderer to stream from. The firmware will provide the same surface
//! over FatFs; nothing above this line (`obc-app`, `obc-render`) knows it's a folder.

use std::path::{Path, PathBuf};

use obc_route::{gpx_to_obcr, ByteSink, Error, RouteStats, RouteSummary, SliceSource};

/// A `ByteSink` over a growable `Vec` — converts a GPX to OBCR bytes in memory before
/// they're written to the folder.
#[derive(Default)]
struct VecSink {
    buf: Vec<u8>,
}

impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), Error> {
        self.buf.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
        let o = off as usize;
        let end = o.checked_add(b.len()).ok_or(Error::BadOffset)?;
        if end > self.buf.len() {
            return Err(Error::BadOffset);
        }
        self.buf[o..end].copy_from_slice(b);
        Ok(())
    }
}

/// The folder-backed route store: the catalog of summaries (for the menu) plus the
/// bytes of the one active route (for the Map to stream).
pub struct RouteStore {
    dir: PathBuf,
    catalog: Vec<RouteSummary>,
    paths: Vec<PathBuf>,
    active: Option<usize>,
    active_bytes: Option<Vec<u8>>,
}

impl RouteStore {
    /// Open and scan the routes folder. A missing folder is fine — it scans to an
    /// empty catalog (the menu shows its empty state); the folder is created lazily on
    /// the first import.
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        let mut s = RouteStore {
            dir: dir.into(),
            catalog: Vec::new(),
            paths: Vec::new(),
            active: None,
            active_bytes: None,
        };
        s.rescan();
        s
    }

    /// The route catalog (summaries), for [`App::set_routes`](obc_app::App::set_routes).
    pub fn catalog(&self) -> &[RouteSummary] {
        &self.catalog
    }

    /// Re-read the folder's `.obcr` files into the catalog (sorted by filename).
    pub fn rescan(&mut self) {
        self.catalog.clear();
        self.paths.clear();
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
                        self.catalog.push(sum);
                        self.paths.push(p);
                    }
                }
            }
        }
        // A reshuffled folder can invalidate the active index; drop it if so.
        if self.active.is_some_and(|i| i >= self.catalog.len()) {
            self.active = None;
            self.active_bytes = None;
        }
    }

    /// Convert a GPX file into the store (named after its file stem) and rescan.
    /// Returns the computed stats — the same conversion the device runs on a USB drop.
    pub fn import_gpx(&mut self, gpx_path: &Path) -> Result<RouteStats, String> {
        let gpx = std::fs::read(gpx_path).map_err(|e| format!("read {}: {e}", gpx_path.display()))?;
        let stem = gpx_path.file_stem().and_then(|s| s.to_str()).unwrap_or("route");
        let mut sink = VecSink::default();
        let stats = gpx_to_obcr(&SliceSource(&gpx), stem, &mut sink)
            .map_err(|e| format!("convert {}: {e:?}", gpx_path.display()))?;
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("create {}: {e}", self.dir.display()))?;
        let out = self.dir.join(format!("{stem}.obcr"));
        std::fs::write(&out, &sink.buf).map_err(|e| format!("write {}: {e}", out.display()))?;
        self.rescan();
        Ok(stats)
    }

    /// Make the active route match `want` (a catalog index), (re)reading its bytes from
    /// disk only when the selection changes. Cheap to call every frame.
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
