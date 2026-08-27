//! Host-side recorded-track storage — the simulator's stand-in for the SD card.
//!
//! Mirrors [`RouteStore`](crate::routes::RouteStore): the shared app decides the ride lifecycle
//! ([`RecorderMachine`](obc_app::RecorderMachine)) and the host performs the operation it names.
//! While riding, each accepted fix appends its final 20-byte representation to an in-progress ride
//! object; a finalize appends the fixed v3 footer and renames the same object into place. A
//! simulator-only GPX convenience export streams the same samples and skips that footer. The save
//! filename is the route that *started* the session, so a later "Swap route only" can't
//! rename a finished file.

use std::path::PathBuf;

use obc_ports::{TrackPoint, TrackSink};
use {
    obc_formats::{io::SliceSource, track::encode_record},
    obc_host_core::VecSink,
    obc_ports::TrackError,
    obc_route::{encode_summary_footer, track_to_gpx, RideStats},
    std::fs::{self, File, OpenOptions},
    std::io::Write,
};

/// An open ride object: the save name (frozen at begin), its private `.obcr.part` path, and the
/// append handle. Implements [`TrackSink`], so a recorded fix lands in it.
struct OpenRide {
    name: String,
    temp: PathBuf,
    file: File,
    point_count: u32,
    first_t_ms: Option<u32>,
}

impl TrackSink for OpenRide {
    fn record(&mut self, p: TrackPoint) -> Result<(), TrackError> {
        // Append the fixed record; a write error surfaces as `Err` so the app raises the
        // recording-error indicator (issue #11) — the ride keeps going regardless.
        self.file.write_all(&encode_record(&p)).map_err(|_| TrackError)?;
        self.first_t_ms.get_or_insert(p.t_ms);
        self.point_count = self.point_count.saturating_add(1);
        Ok(())
    }
}

/// The simulator's recorded-ride store: a folder of saved `.gpx` files plus at most one in-progress
/// v3 ride object.
pub struct TrackStore {
    dir: PathBuf,
    open: Option<OpenRide>,
}

impl TrackStore {
    /// Open (creating) the tracks folder `dir`.
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let _ = fs::create_dir_all(&dir);
        TrackStore { dir, open: None }
    }

    /// The [`TrackSink`] for the open ride, or `None` when nothing is recording.
    pub fn sink(&mut self) -> Option<&mut dyn TrackSink> {
        self.open.as_mut().map(|o| o as &mut dyn TrackSink)
    }

    /// Open a fresh private `.obcr.part` object for session `id`, to be saved as `name`.
    fn begin(&mut self, id: u32, name: &str) {
        self.open = None; // close any previous handle first
        let temp = self.dir.join(format!(".ride-{id}.obcr.part"));
        match OpenOptions::new().create(true).write(true).truncate(true).open(&temp) {
            Ok(file) => {
                self.open = Some(OpenRide { name: name.to_string(), temp, file, point_count: 0, first_t_ms: None })
            }
            Err(e) => eprintln!("track: cannot open ride {}: {e}", temp.display()),
        }
    }

    /// Finalise the open object: append the fixed footer and rename the same bytes into the
    /// simulator-only `ride-{id}.obcr` namespace. Answers with the identity it committed, or `None`
    /// when the footer or the rename failed — the ride is then still there and Recorder retries.
    fn finalize(&mut self, stats: RideStats) -> Option<u64> {
        let mut log = self.open.take()?;
        let mut committed = None;
        let footer = encode_summary_footer(&log.name, &stats, log.point_count, log.first_t_ms);
        let footer_written = log.file.write_all(&footer).and_then(|()| log.file.flush()).is_ok();
        let _ = log.file.flush();
        drop(log.file);

        let mut object_path = log.temp.clone();
        if footer_written {
            match self.allocate_fixture_object_id() {
                Some(id) => {
                    let final_path = self.dir.join(format!("ride-{id}.obcr"));
                    match fs::rename(&log.temp, &final_path) {
                        Ok(()) => {
                            eprintln!("track: saved ride {}", final_path.display());
                            object_path = final_path;
                            committed = Some(id);
                        }
                        Err(e) => eprintln!("track: cannot rename {}: {e}", log.temp.display()),
                    }
                }
                None => eprintln!("track: desktop ride object id namespace is exhausted"),
            }
        }

        let bytes = match fs::read(&object_path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("track: cannot read ride {}: {e}", object_path.display());
                if object_path == log.temp {
                    let _ = fs::remove_file(&log.temp);
                }
                return committed;
            }
        };
        // The convenience `.gpx` (sim-only).
        let mut sink = VecSink::default();
        if track_to_gpx(&SliceSource(&bytes), &log.name, &mut sink).is_ok() {
            let path = self.unique_gpx(&log.name);
            match fs::write(&path, sink.bytes()) {
                Ok(()) => eprintln!("track: saved {}", path.display()),
                Err(e) => eprintln!("track: cannot write {}: {e}", path.display()),
            }
        }
        if object_path == log.temp {
            let _ = fs::remove_file(&log.temp);
        }
        committed
    }

    /// Allocate a non-clobbering full-width key for the desktop fixture namespace. This scan is only
    /// local filesystem bookkeeping; it does not emulate the device's flat-store id allocator.
    fn allocate_fixture_object_id(&self) -> Option<u64> {
        let mut next = 0u64;
        if let Ok(rd) = fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                if let Some(id) = e
                    .path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_prefix("ride-"))
                    .and_then(|n| n.strip_suffix(".obcr"))
                    .and_then(|n| n.parse::<u64>().ok())
                {
                    next = next.max(id.checked_add(1)?);
                }
            }
        }
        Some(next)
    }

    /// Drop the open ride without saving (Discard, or a no-session reconcile).
    fn abandon(&mut self) {
        if let Some(log) = self.open.take() {
            drop(log.file);
            let _ = fs::remove_file(&log.temp);
        }
    }

    /// A non-clobbering `<name>.gpx` path. Timestamps land later; for now a numeric suffix
    /// keeps a re-ridden route from silently overwriting an earlier save.
    fn unique_gpx(&self, name: &str) -> PathBuf {
        let stem = sanitize(name);
        let first = self.dir.join(format!("{stem}.gpx"));
        if !first.exists() {
            return first;
        }
        (2..=9999).map(|n| self.dir.join(format!("{stem} ({n}).gpx"))).find(|p| !p.exists()).unwrap_or(first)
    }
}

/// The shared dispatcher ([`obc_host_core::HostLoop`]) performs each recording operation through
/// this trait, so what a ride *is* stays Recorder's and what it costs stays the store's.
impl obc_host_core::TrackRepository for TrackStore {
    fn open(&mut self, session: u32, name: Option<&str>) {
        self.begin(session, name.unwrap_or("ride"));
    }

    fn finalize(&mut self, stats: RideStats) -> Option<obc_app::CatalogObjectId> {
        self.finalize(stats)
    }

    fn discard(&mut self) {
        self.abandon();
    }

    fn sink(&mut self) -> Option<&mut dyn TrackSink> {
        self.sink()
    }
}

/// Replace path separators / control chars so a route name is a safe filename stem.
fn sanitize(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_control() || matches!(c, '/' | '\\') { '_' } else { c }).collect();
    let trimmed = s.trim();
    if trimmed.is_empty() {
        "ride".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The **folder-backed** track store passes the shared `obc-host-core` track-lifecycle
    /// conformance (a session opens a ride, Save/Discard closes it, a live session wins over a
    /// drained action) — it exposes a real recording sink, so `has_sink = true`.
    #[test]
    fn folder_track_store_passes_the_lifecycle_suite() {
        let dir = obcm_testkit::scratch::scratch_dir("obc-track-conf", "lifecycle");
        let mut store = TrackStore::open(&dir);
        obc_host_core::conformance::track_lifecycle(&mut store, true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fixture_allocator_is_full_width_and_ignores_the_deleted_fat_namespace() {
        let dir = obcm_testkit::scratch::scratch_dir("obc-track-id", "full-width");
        std::fs::write(dir.join("ride-65536.obcr"), []).unwrap();
        std::fs::write(dir.join("notes.txt"), []).unwrap();
        let store = TrackStore::open(&dir);
        assert_eq!(store.allocate_fixture_object_id(), Some(65_537));

        std::fs::write(dir.join(format!("ride-{}.obcr", u64::MAX)), []).unwrap();
        assert_eq!(store.allocate_fixture_object_id(), None, "the allocator never wraps and clobbers ride-0");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
