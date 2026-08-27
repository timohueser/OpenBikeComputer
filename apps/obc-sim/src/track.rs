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

use obc_app::recorder::RideClose;
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
    /// The fixed v3 footer is already appended. A finalize that got this far and then failed to
    /// commit must not append it twice on the retry — storage sees one footer per ride, and the
    /// retry has to be the same close.
    footer_written: bool,
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
                self.open = Some(OpenRide {
                    name: name.to_string(),
                    temp,
                    file,
                    point_count: 0,
                    first_t_ms: None,
                    footer_written: false,
                })
            }
            Err(e) => eprintln!("track: cannot open ride {}: {e}", temp.display()),
        }
    }

    /// Finalise the open object: append the fixed v3 footer and rename the same bytes into the
    /// simulator-only `ride-{id}.obcr` namespace. Answers with the identity it committed, or `None`
    /// when it could not commit.
    ///
    /// **`None` leaves the ride exactly where it was**, and that is the contract, not a nicety:
    /// Recorder keeps the close pending and offers the same finalize again, so a store that
    /// discarded the bytes on the way out would answer `None` for ever and the rider's ride would
    /// be gone with a recording warning on every pass. Nothing is deleted until the rename commits;
    /// `footer_written` is what makes the retry the same close rather than a second footer.
    fn finalize(&mut self, stats: RideStats) -> RideClose {
        let Some(log) = self.open.as_mut() else { return RideClose::Nothing };
        if !log.footer_written {
            let footer = encode_summary_footer(&log.name, &stats, log.point_count, log.first_t_ms);
            if let Err(e) = log.file.write_all(&footer).and_then(|()| log.file.flush()) {
                eprintln!("track: cannot write the footer for {}: {e}", log.temp.display());
                return RideClose::Failed; // the samples are untouched; the retry writes it again
            }
            log.footer_written = true;
        }
        let Some(id) = self.allocate_fixture_object_id() else {
            eprintln!("track: desktop ride object id namespace is exhausted");
            return RideClose::Failed;
        };
        let log = self.open.as_ref().expect("still open");
        let final_path = self.dir.join(format!("ride-{id}.obcr"));
        if let Err(e) = fs::rename(&log.temp, &final_path) {
            eprintln!("track: cannot rename {}: {e}", log.temp.display());
            return RideClose::Failed; // the object is still there under its private name
        }
        eprintln!("track: saved ride {}", final_path.display());

        // Committed. Only now does the open ride stop being the store's to retry.
        let log = self.open.take().expect("still open");
        drop(log.file);
        // The convenience `.gpx` (sim-only). A failure here loses nothing durable: the ride object
        // is committed either way, so the close has already succeeded.
        match fs::read(&final_path) {
            Ok(bytes) => {
                let mut sink = VecSink::default();
                if track_to_gpx(&SliceSource(&bytes), &log.name, &mut sink).is_ok() {
                    let path = self.unique_gpx(&log.name);
                    match fs::write(&path, sink.bytes()) {
                        Ok(()) => eprintln!("track: saved {}", path.display()),
                        Err(e) => eprintln!("track: cannot write {}: {e}", path.display()),
                    }
                }
            }
            Err(e) => eprintln!("track: cannot read ride {}: {e}", final_path.display()),
        }
        RideClose::Committed(id)
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

    fn finalize(&mut self, stats: RideStats) -> RideClose {
        self.finalize(stats)
    }

    fn discard(&mut self) -> bool {
        self.abandon();
        true
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

    /// **A finalize that cannot commit keeps the ride, so the retry is the same close.**
    ///
    /// The contract `TrackRepository::finalize` states, forced on the real folder store. Recorder
    /// holds the rider's Save pending until a terminal verdict, so a store that removed the bytes
    /// on the way out would answer `None` for ever: the ride gone, `REC_ERROR` on every pass, and
    /// no way back. The failure is forced by taking the object's path away, which is what a rename
    /// into a folder the sim no longer owns looks like.
    #[test]
    fn a_finalize_that_cannot_commit_keeps_the_ride() {
        use obc_host_core::TrackRepository;
        let dir = obcm_testkit::scratch::scratch_dir("obc-track-retry", "failed-finalize");
        let mut store = TrackStore::open(&dir);
        store.open(1, Some("Ride"));
        store
            .sink()
            .expect("a folder store records")
            .record(TrackPoint {
                lon: 8_000_000,
                lat: 46_000_000,
                ele: 1000,
                t_ms: 0,
                segment_start: true,
                hr: None,
                cadence: None,
                power: None,
            })
            .unwrap();

        // The object's own path is taken away: the footer still writes through the open handle, and
        // the rename that commits it cannot find its source.
        let temp = dir.join(".ride-1.obcr.part");
        std::fs::remove_file(&temp).unwrap();
        let first = TrackRepository::finalize(&mut store, ride_stats());
        assert_eq!(first, RideClose::Failed, "the close did not commit");
        assert!(store.sink().is_some(), "and the ride is still the store's to retry");
        let again = TrackRepository::finalize(&mut store, ride_stats());
        assert_eq!(again, RideClose::Failed, "the retry is the same close, not a new failure mode");
        assert!(store.sink().is_some(), "which leaves the ride exactly where it was");

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn ride_stats() -> RideStats {
        RideStats {
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
        }
    }

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
