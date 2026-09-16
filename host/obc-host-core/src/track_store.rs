//! Recording goes to the session card; a committed ride is also exported as a GPX file.

use crate::{flat_store::HostStore, AppendStatus, FlatRideRecorder, TrackRepository, VecSink};
use obc_app::recorder::{CheckpointStatus, RecorderError, RideClose, RideContinuation};
use obc_ports::TrackPoint;
use obc_route::{RideInfo, RideStats};
use obc_storage::flat::{ObjectId, Revision};
use std::path::PathBuf;

/// How many names one ride id can take in the exports directory before the export gives up.
const EXPORT_NAMES: u32 = 1000;

pub struct TrackStore {
    recorder: FlatRideRecorder,
    owner: HostStore,
    exports: PathBuf,
}

impl TrackStore {
    pub fn new(recorder: FlatRideRecorder, owner: HostStore, exports: impl Into<PathBuf>) -> Self {
        Self { recorder, owner, exports: exports.into() }
    }

    pub fn offer_recovery(&self, app: &mut obc_app::App) {
        self.recorder.offer_recovery(app);
    }

    fn export(&self, id: u64) -> Result<(), String> {
        let source = self.owner.open(ObjectId(id), Revision(1)).map_err(|error| format!("{error:?}"))?;
        let info = RideInfo::read(&source).map_err(|error| format!("{error:?}"))?;
        let mut sink = VecSink::default();
        obc_route::track_to_gpx(&source, info.name.as_str(), &mut sink).map_err(|error| format!("{error:?}"))?;
        std::fs::create_dir_all(&self.exports).map_err(|error| error.to_string())?;
        // A reset card restarts its ride ids while the exports directory survives, so a taken name
        // takes the next free suffix instead of losing the file the rider was told was saved.
        let mut path = self.exports.join(format!("ride-{id}.gpx"));
        let mut opened = None;
        for next in 1..EXPORT_NAMES {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    opened = Some(file);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    path = self.exports.join(format!("ride-{id}-{next}.gpx"));
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        use std::io::Write;
        let mut file = opened.ok_or_else(|| format!("ride {id}: no free export name in {}", self.exports.display()))?;
        file.write_all(sink.bytes()).map_err(|error| error.to_string())?;
        eprintln!("track: exported {}", path.display());
        Ok(())
    }
}

impl TrackRepository for TrackStore {
    fn open(&mut self, session: u32, name: Option<&str>, now_ms: u32) -> bool {
        self.recorder.open(session, name, now_ms)
    }
    fn append_batch(
        &mut self,
        points: &[TrackPoint],
        continuation: Option<RideContinuation>,
    ) -> Result<AppendStatus, RecorderError> {
        self.recorder.append_batch(points, continuation)
    }
    fn checkpoint(
        &mut self,
        stats: RideStats,
        continuation: Option<RideContinuation>,
    ) -> Result<CheckpointStatus, RecorderError> {
        self.recorder.checkpoint(stats, continuation)
    }
    fn finalize(&mut self, stats: RideStats) -> RideClose {
        let result = self.recorder.finalize(stats);
        if let RideClose::Committed(id) = result {
            eprintln!("track: committed card ride {id} revision 1");
            if let Err(error) = self.export(id) {
                eprintln!("track: GPX export failed after Save: {error}");
            }
        }
        result
    }
    fn discard(&mut self) -> Result<(), RecorderError> {
        self.recorder.discard()
    }
    fn append(&mut self, _point: TrackPoint) -> bool {
        false
    }
}
