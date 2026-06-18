//! Host-side recorded-track storage — the simulator's stand-in for the SD card.
//!
//! Mirrors [`RouteStore`](crate::routes::RouteStore): the shared app expresses *intent* (an
//! active [`session`](obcm_app::Activity::session) id + a one-shot
//! [`TrackAction`](obcm_app::TrackAction)) and the host reconciles it to files here each
//! frame. While riding, every accepted fix is appended to a temp `.obct` log (the firmware
//! would append to a FatFs file); on Finish the log is converted to a `.gpx` via
//! [`obcm_route::track_to_gpx`] and the temp dropped; on Discard the temp is dropped
//! unconverted. The save filename is the route that *started* the session, so a later "Swap
//! route only" can never rename a finished file.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use obcm_app::{TrackAction, TrackSink};
use obcm_route::{encode_record, track_to_gpx, ByteSink, Error, SliceSource, TrackPoint};

/// An open ride log: the session it belongs to, the save name (frozen at begin), its temp
/// `.obct` path, and the append handle. Implements [`TrackSink`], so `App::tick` logs to it.
struct OpenLog {
    id: u32,
    name: String,
    temp: PathBuf,
    file: File,
}

impl TrackSink for OpenLog {
    fn record(&mut self, p: TrackPoint) {
        // Append the fixed record; a write error just drops the point (the ride continues).
        let _ = self.file.write_all(&encode_record(&p));
    }
}

/// The simulator's recorded-track store: a folder of saved `.gpx` files plus at most one open
/// `.obct` log.
pub struct TrackStore {
    dir: PathBuf,
    open: Option<OpenLog>,
}

impl TrackStore {
    /// Open (creating) the tracks folder `dir`.
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let _ = fs::create_dir_all(&dir);
        TrackStore { dir, open: None }
    }

    /// Reconcile the open log to the app's tracking intent — call once per frame *before*
    /// ticking. `action` is the drained one-shot, `session` the current id, `name` the active
    /// route's name (the save filename). Drains the action first (finalising / abandoning the
    /// *current* log), then opens a fresh log when the session id changes.
    pub fn reconcile(&mut self, action: Option<TrackAction>, session: Option<u32>, name: Option<&str>) {
        match action {
            Some(TrackAction::Save) => self.finalize(),
            Some(TrackAction::Discard) => self.abandon(),
            None => {}
        }
        match session {
            Some(id) if self.open.as_ref().map(|o| o.id) != Some(id) => {
                self.begin(id, name.unwrap_or("ride"))
            }
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

    /// Finalise the open log to `<name>.gpx` and drop the temp.
    fn finalize(&mut self) {
        let Some(mut log) = self.open.take() else { return };
        let _ = log.file.flush();
        drop(log.file);
        match fs::read(&log.temp) {
            Ok(bytes) => {
                let mut sink = VecSink::default();
                if track_to_gpx(&SliceSource(&bytes), &log.name, &mut sink).is_ok() {
                    let path = self.unique_gpx(&log.name);
                    match fs::write(&path, &sink.0) {
                        Ok(()) => eprintln!("track: saved {}", path.display()),
                        Err(e) => eprintln!("track: cannot write {}: {e}", path.display()),
                    }
                }
            }
            Err(e) => eprintln!("track: cannot read log {}: {e}", log.temp.display()),
        }
        let _ = fs::remove_file(&log.temp);
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
        (2..=9999)
            .map(|n| self.dir.join(format!("{stem} ({n}).gpx")))
            .find(|p| !p.exists())
            .unwrap_or(first)
    }
}

/// Replace path separators / control chars so a route name is a safe filename stem.
fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\') { '_' } else { c })
        .collect();
    let trimmed = s.trim();
    if trimmed.is_empty() { "ride".to_string() } else { trimmed.to_string() }
}

/// A `ByteSink` collecting the GPX into a `Vec` before one `fs::write` (mirrors the route
/// store's in-memory conversion; a ride's GPX is a few MB at most on the host).
#[derive(Default)]
struct VecSink(Vec<u8>);
impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), Error> {
        self.0.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
        let o = off as usize;
        self.0[o..o + b.len()].copy_from_slice(b);
        Ok(())
    }
}
