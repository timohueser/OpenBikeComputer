//! Host-side recorded-track storage — the simulator's stand-in for the SD card.
//!
//! Mirrors [`RouteStore`](crate::routes::RouteStore): the shared app expresses *intent* (an
//! active [`session`](obc_app::Activity::session) id + a one-shot
//! [`TrackAction`](obc_app::TrackAction)) and the host reconciles it to files each frame.
//! While riding, each accepted fix appends to a temp `.obct` log; on Finish it's converted
//! to `.gpx` via [`obc_route::track_to_gpx`], on Discard dropped unconverted. The save
//! filename is the route that *started* the session, so a later "Swap route only" can't
//! rename a finished file.

use std::path::PathBuf;

use obc_app::{TrackAction, TrackSink};
#[cfg(not(target_arch = "wasm32"))]
use {
    crate::vec_sink::VecSink,
    obc_route::{encode_record, track_to_gpx, track_to_ride, RideStats, SliceSource, TrackPoint},
    std::fs::{self, File, OpenOptions},
    std::io::Write,
};

/// An open ride log: the session it belongs to, the save name (frozen at begin), its temp
/// `.obct` path, and the append handle. Implements [`TrackSink`], so `App::tick` logs to it.
#[cfg(not(target_arch = "wasm32"))]
struct OpenLog {
    id: u32,
    name: String,
    temp: PathBuf,
    file: File,
}

#[cfg(not(target_arch = "wasm32"))]
impl TrackSink for OpenLog {
    fn record(&mut self, p: TrackPoint) {
        // Append the fixed record; a write error just drops the point (the ride continues).
        let _ = self.file.write_all(&encode_record(&p));
    }
}

/// The simulator's recorded-track store: a folder of saved `.gpx` files plus at most one open
/// `.obct` log.
#[cfg(not(target_arch = "wasm32"))]
pub struct TrackStore {
    dir: PathBuf,
    open: Option<OpenLog>,
}

#[cfg(not(target_arch = "wasm32"))]
impl TrackStore {
    /// Open (creating) the tracks folder `dir`.
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let _ = fs::create_dir_all(&dir);
        TrackStore { dir, open: None }
    }

    /// Reconcile the open log to the app's tracking intent — call once per frame *before*
    /// ticking. Drains the action first (finalising / abandoning the *current* log), then opens
    /// a fresh log when the session id changes. `name` is the save filename; `stats` are the app's
    /// ride totals at Finish (from [`App::ride_stats`](obc_app::App::ride_stats)) — needed to write
    /// the durable `RD{id}.ORD` ride object the Rides screen lists, exactly as the device does.
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

    /// The [`TrackSink`] for the open log, or `None` when nothing is recording.
    pub fn sink(&mut self) -> Option<&mut dyn TrackSink> {
        self.open.as_mut().map(|o| o as &mut dyn TrackSink)
    }

    /// Whether a ride is currently being recorded.
    pub fn is_recording(&self) -> bool {
        self.open.is_some()
    }

    /// Open a fresh temp `.obct` for session `id`, to be saved as `name`. Drops any prior log.
    fn begin(&mut self, id: u32, name: &str) {
        self.open = None; // close any previous handle first
        let temp = self.dir.join(format!(".track-{id}.obct"));
        match OpenOptions::new().create(true).write(true).truncate(true).open(&temp) {
            Ok(file) => self.open = Some(OpenLog { id, name: name.to_string(), temp, file }),
            Err(e) => eprintln!("track: cannot open log {}: {e}", temp.display()),
        }
    }

    /// Finalise the open log and drop the temp. Writes the durable `RD{id}.ORD` ride object (the
    /// device's Finish artifact — what the Rides screen lists) when `stats` are supplied, *and* a
    /// human-readable `<name>.gpx` (the sim's legacy convenience export; the device no longer writes
    /// GPX). With no stats (only possible on the headless `--save-track` path) it keeps just the GPX.
    fn finalize(&mut self, stats: Option<RideStats>) {
        let Some(mut log) = self.open.take() else { return };
        let _ = log.file.flush();
        drop(log.file);
        let bytes = match fs::read(&log.temp) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("track: cannot read log {}: {e}", log.temp.display());
                let _ = fs::remove_file(&log.temp);
                return;
            }
        };
        // The ride object `RD{id}.ORD` — the durable artifact the Rides screen scans (issue #454).
        if let Some(stats) = stats {
            let id = self.next_ride_id();
            let mut sink = VecSink::default();
            if track_to_ride(&SliceSource(&bytes), &log.name, &stats, &mut sink).is_ok() {
                let path = self.dir.join(format!("RD{id}.ORD"));
                match fs::write(&path, sink.bytes()) {
                    Ok(()) => eprintln!("track: saved ride {}", path.display()),
                    Err(e) => eprintln!("track: cannot write {}: {e}", path.display()),
                }
            }
        }
        // The convenience `.gpx` (sim-only).
        let mut sink = VecSink::default();
        if track_to_gpx(&SliceSource(&bytes), &log.name, &mut sink).is_ok() {
            let path = self.unique_gpx(&log.name);
            match fs::write(&path, sink.bytes()) {
                Ok(()) => eprintln!("track: saved {}", path.display()),
                Err(e) => eprintln!("track: cannot write {}: {e}", path.display()),
            }
        }
        let _ = fs::remove_file(&log.temp);
    }

    /// The next unused `RD{id}.ORD` id — one past the highest already in the tracks folder (0 on a
    /// virgin folder). Mirrors the device's scan-based ride-id allocation; the sim doesn't persist a
    /// high-water floor, which is fine (it never reflashes mid-session).
    fn next_ride_id(&self) -> u16 {
        let mut next = 0u16;
        if let Ok(rd) = fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                if let Some(id) = e
                    .path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_prefix("RD"))
                    .and_then(|n| n.strip_suffix(".ORD"))
                    .and_then(|n| n.parse::<u16>().ok())
                {
                    next = next.max(id + 1);
                }
            }
        }
        next
    }

    /// Drop the open log without saving (Discard, or a no-session reconcile).
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

/// Replace path separators / control chars so a route name is a safe filename stem.
#[cfg(not(target_arch = "wasm32"))]
fn sanitize(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_control() || matches!(c, '/' | '\\') { '_' } else { c }).collect();
    let trimmed = s.trim();
    if trimmed.is_empty() {
        "ride".to_string()
    } else {
        trimmed.to_string()
    }
}

// --- Web (wasm32) track store ---------------------------------------------------
//
// No filesystem, so no on-disk log: the breadcrumb + ride stats come from the shared app
// state, not this sink. It only tracks whether a ride is active so `is_recording()` stays
// honest. Downloadable-`.gpx` save is a later addition.
#[cfg(target_arch = "wasm32")]
pub struct TrackStore {
    recording: bool,
}

#[cfg(target_arch = "wasm32")]
impl TrackStore {
    /// `dir` is ignored on the web; the signature matches the native store.
    pub fn open(_dir: impl Into<PathBuf>) -> Self {
        TrackStore { recording: false }
    }

    /// Mirror the native reconcile's recording flag without touching a filesystem:
    /// a drained Save/Discard ends the ride, then a live session id (re)starts it. `stats` are unused
    /// on the web (no ride object is written — the fixed catalog seeds the Rides list instead).
    pub fn reconcile(
        &mut self,
        action: Option<TrackAction>,
        session: Option<u32>,
        _name: Option<&str>,
        _stats: Option<obc_route::RideStats>,
    ) {
        if matches!(action, Some(TrackAction::Save) | Some(TrackAction::Discard)) {
            self.recording = false;
        }
        self.recording = session.is_some();
    }

    /// No persistent sink on the web — the app still draws the live breadcrumb itself.
    pub fn sink(&mut self) -> Option<&mut dyn TrackSink> {
        None
    }

    pub fn is_recording(&self) -> bool {
        self.recording
    }
}
