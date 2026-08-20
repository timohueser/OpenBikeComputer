//! Host-side recorded-track storage — the simulator's stand-in for the SD card.
//!
//! Mirrors [`RouteStore`](crate::routes::RouteStore): the shared app expresses *intent* (an
//! active [`session`](obc_app::Activity::session) id + a one-shot
//! [`TrackAction`](obc_app::TrackAction)) and the host reconciles it to files each frame.
//! While riding, each accepted fix appends its final 20-byte representation to an in-progress ride
//! object; on Finish the fixed v3 footer is appended and the same object is renamed into place. A
//! simulator-only GPX convenience export streams the same samples and skips that footer. The save
//! filename is the route that *started* the session, so a later "Swap route only" can't
//! rename a finished file.

use std::path::PathBuf;

use obc_app::TrackAction;
use obc_ports::{TrackPoint, TrackSink};
use {
    obc_formats::{io::SliceSource, track::encode_record},
    obc_host_core::VecSink,
    obc_ports::TrackError,
    obc_route::{encode_summary_footer, track_to_gpx, RideStats},
    std::fs::{self, File, OpenOptions},
    std::io::Write,
};

/// An open ride object: the session it belongs to, the save name (frozen at begin), its private
/// `.obcr.part` path, and the append handle. Implements [`TrackSink`], so `App::tick` records to it.
struct OpenRide {
    id: u32,
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

    /// Reconcile the open ride to the app's tracking intent — call once per frame *before*
    /// ticking. Drains the action first (finalising / abandoning the current ride), then opens
    /// a fresh object when the session id changes. `name` is the save filename; `stats` are the app's
    /// ride totals at Finish (from [`App::ride_stats`](obc_app::App::ride_stats)) — needed to append
    /// the same v3 footer the device records. The surrounding filename is simulator-only.
    pub fn reconcile(
        &mut self,
        action: Option<TrackAction>,
        session: Option<u32>,
        name: Option<&str>,
        stats: Option<RideStats>,
    ) {
        match action {
            Some(TrackAction::Save) => self.finalize(stats),
            Some(TrackAction::Discard) => self.abandon(),
            None => {}
        }
        match session {
            Some(id) if self.open.as_ref().map(|o| o.id) != Some(id) => self.begin(id, name.unwrap_or("ride")),
            None => self.abandon(), // no session → ensure nothing is left open
            _ => {}                 // same session → keep appending
        }
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
                self.open = Some(OpenRide { id, name: name.to_string(), temp, file, point_count: 0, first_t_ms: None })
            }
            Err(e) => eprintln!("track: cannot open ride {}: {e}", temp.display()),
        }
    }

    /// Finalise the open object. With stats, append the fixed footer and rename the same bytes into
    /// the simulator-only `ride-{id}.obcr` namespace; with no stats, keep only the GPX export.
    fn finalize(&mut self, stats: Option<RideStats>) {
        let Some(mut log) = self.open.take() else { return };
        let footer_written = stats.is_some_and(|stats| {
            let footer = encode_summary_footer(&log.name, &stats, log.point_count, log.first_t_ms);
            log.file.write_all(&footer).and_then(|()| log.file.flush()).is_ok()
        });
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
                return;
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

/// The shared dispatcher ([`obc_host_core::HostLoop`]) reconciles the ride recorder through this trait,
/// so the finalise/abandon/begin lifecycle order lives in one place for every host.
impl obc_host_core::TrackRepository for TrackStore {
    fn reconcile(
        &mut self,
        action: Option<TrackAction>,
        session: Option<u32>,
        name: Option<&str>,
        stats: Option<RideStats>,
    ) {
        self.reconcile(action, session, name, stats)
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
