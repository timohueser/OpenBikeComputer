//! Durable desktop ride library. [`Library::import`] returns only after it has flushed
//! the ride object, GPX, and index in that order.
//!
//! ```text
//!   <library>/                            rider-owned, relocatable GPX files
//!     2026-07-20-schauinsland.gpx
//!
//!   <app data>/ride-archive/              internal, fixed-location archive
//!     index.json
//!     2026-07-20-schauinsland.obcride
//! ```
//!
//! The lossless `.obcride` copy makes GPX re-export possible. Identity is
//! `(serial, store_id, id)` because object ids can be reused after a card change.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Serializes imports and relocation so temp names and index updates cannot interleave.
static LIBRARY_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    // A panic while holding the lock poisons it; the disk state is still governed by the
    // rename-last discipline, so the next operation may simply proceed.
    LIBRARY_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The index's filename. It is readable JSON on purpose: it is the record a person can open and
/// read.
pub const INDEX_FILE: &str = "index.json";
/// Bumped only when an older index can no longer be read. An unreadable index is not fatal: it
/// re-imports and never deletes.
const INDEX_VERSION: u32 = 2;
/// The stored ride object's extension, chosen so it cannot be mistaken for a GPX.
const RIDE_EXT: &str = "obcride";
const GPX_EXT: &str = "gpx";

/// Largest ride object this command accepts. It is a ceiling and not an expectation: it is roughly
/// ten days of continuous recording, and it exists because a Tauri command is a door.
const MAX_RIDE_BYTES: usize = 16 * 1024 * 1024;
/// The GPX is the same track as text, five to six times the object, so its ceiling keeps the same
/// margin.
const MAX_GPX_BYTES: usize = 128 * 1024 * 1024;
/// Preview tracks are drawn a few hundred pixels wide, so more points than this would be index
/// weight nobody can see. The frontend downsamples to it and this is the enforcement.
const MAX_TRACK_POINTS: usize = 512;
/// Longest filename stem the library will mint, before its extension.
const MAX_STEM: usize = 64;

/// The pointer file that remembers a relocated library. It lives in the app's config directory: a
/// folder that named itself could not be found once it moved.
const LOCATION_FILE: &str = "ride-library.json";

/// One ride in the library, as `index.json` stores it.
///
/// It holds only facts that stay true: no absolute paths, because the GPX folder can move, and no
/// existence, because a person can delete a GPX in the file manager. Those are [`RideEntry`]'s,
/// recomputed on every read.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRide {
    /// `serial:storeId:objectId`, minted here and never taken from the caller.
    pub key: String,
    pub serial: String,
    #[serde(deserialize_with = "deserialize_store_id")]
    pub store_id: String,
    #[serde(serialize_with = "serialize_object_id", deserialize_with = "deserialize_object_id")]
    pub object_id: u64,
    pub name: String,
    /// Ride start, unix seconds UTC. `0` on a device that never had a trusted clock.
    pub start_time: u32,
    pub distance_m: u32,
    pub moving_time_s: u32,
    pub climb_m: u32,
    pub points: u32,
    /// Length of the stored ride object, bytes.
    pub bytes: u64,
    /// The device's whole-object CRC-32, kept so the stored copy can be re-checked without the
    /// device.
    pub crc32: u32,
    /// When this app first landed the ride, unix seconds. Never re-stamped, so a second pull is a
    /// no-op, which matches the first-ack-wins rule on the device.
    pub imported_at: u64,
    /// Basename of the archived ride object, in the archive directory.
    pub ride_file: String,
    /// Basename of the GPX, in the visible folder.
    pub gpx_file: String,
    /// A downsampled `[lat, lon]` track for the list's preview, in degrees, drawn from the ride's
    /// own points.
    pub track: Vec<[f64; 2]>,
}

/// One ride as the UI reads it: the stored record plus what only the filesystem can say.
///
/// The two extra pairs are recomputed on every read and never written down. `present` means the
/// archive file exists: a ride whose object is gone is not a durable copy, so it is not acked and
/// it is pulled again. A missing GPX is only a re-export away, because the archive is its source.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RideEntry {
    #[serde(flatten)]
    pub ride: LibraryRide,
    pub ride_path: String,
    pub gpx_path: String,
    pub present: bool,
    pub gpx_present: bool,
}

/// So an entry reads as the ride it describes. The wrapper adds facts about the filesystem; it is
/// not a different kind of thing.
impl std::ops::Deref for RideEntry {
    type Target = LibraryRide;

    fn deref(&self) -> &LibraryRide {
        &self.ride
    }
}

/// The index file's whole body.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct Index {
    version: u32,
    rides: Vec<LibraryRide>,
}

/// What the UI is handed: where the visible folder is, and what is in the library.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct IndexView {
    pub folder: String,
    /// True when the folder is the app's default rather than one the user picked.
    pub is_default: bool,
    pub rides: Vec<RideEntry>,
}

/// One ride, as the pull hands it over. The bytes and the GPX both cross the IPC boundary here.
///
/// A ride is orders of magnitude smaller than the maps and firmware images that need the raw-body
/// path in [`crate::usb`], and the GPX beside it is text, so a structured command with a stated
/// ceiling is the cheaper thing to review.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    pub serial: String,
    #[serde(deserialize_with = "deserialize_store_id")]
    pub store_id: String,
    #[serde(deserialize_with = "deserialize_object_id")]
    pub object_id: u64,
    pub name: String,
    pub start_time: u32,
    pub distance_m: u32,
    pub moving_time_s: u32,
    pub climb_m: u32,
    /// Points in the recorded track, from the caller's decode of the object. This crate has no
    /// ride-object decoder and must not grow one: the codecs live once, in `lib/usb/objects.ts`.
    pub points: u32,
    pub crc32: u32,
    pub track: Vec<[f64; 2]>,
    /// The ride object exactly as it came off the wire.
    pub object: Vec<u8>,
    /// The GPX 1.1 document, from the same `obc_route::track_to_gpx` the device runs. There is no
    /// GPX writer in this crate and there must never be one.
    pub gpx: String,
}

/// [`Library::import`]'s answer. `imported` is false when the ride was already in the library,
/// which is the idempotent case and must not re-stamp anything.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    pub ride: RideEntry,
    pub imported: bool,
}

/// Where a durable write is interrupted.
///
/// Production constructs a [`Library`] with [`CrashPoint::None`], and nothing outside this module
/// can ask for anything else. The other variants exist so the rule that the ack follows the fsync
/// is checked by running the real code with the power cut at a chosen instant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CrashPoint {
    #[default]
    None,
    /// The bytes reached `write()` and the process died before `fsync`: the case that decides
    /// whether this feature can lose a ride.
    BeforeObjectFsync,
    /// The ride's two files are durable and the process died before the index committed.
    BeforeIndexCommit,
}

/// The message a simulated crash returns, so a test can tell it from a real IO error.
const CRASH_MSG: &str = "simulated power loss";

pub struct Library {
    /// The visible, relocatable GPX folder.
    root: PathBuf,
    /// The internal archive: `index.json` plus the ride objects. It is app data, so it does not
    /// move when the rider moves the GPX folder.
    archive: PathBuf,
    crash: CrashPoint,
}

impl Library {
    pub fn new(root: PathBuf, archive: PathBuf) -> Self {
        Library { root, archive, crash: CrashPoint::None }
    }

    /// A library that dies at `crash`. Test-only by construction, not by convention.
    #[cfg(test)]
    fn crashing_at(root: PathBuf, archive: PathBuf, crash: CrashPoint) -> Self {
        Library { root, archive, crash }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The index as it is on disk right now, with existence recomputed per ride.
    ///
    /// A missing index is an empty library, and so is a corrupt one. That is the safe direction:
    /// the worst it costs is re-downloading rides the device still holds, while salvaging a
    /// half-parsed index risks writing over the files it half-understood.
    fn load(&self) -> Index {
        read_index(&self.archive.join(INDEX_FILE))
            .unwrap_or_else(|| Index { version: INDEX_VERSION, rides: Vec::new() })
    }

    /// A stored record, joined to the filesystem as it is right now.
    fn entry(&self, ride: LibraryRide) -> RideEntry {
        let ride_path = self.archive.join(&ride.ride_file);
        let gpx_path = self.root.join(&ride.gpx_file);
        // Existence alone is not durability. `present` feeds the ack, so a truncated or swapped
        // archive file must read as absent: the ride is then pulled again, which costs a download
        // instead of a ride. The index's own `bytes` is the cheap whole-file check; hashing every
        // archive on every read would make listing a library cost its size.
        let present = std::fs::metadata(&ride_path).is_ok_and(|m| m.is_file() && m.len() == ride.bytes);
        RideEntry {
            present,
            gpx_present: gpx_path.is_file(),
            ride_path: ride_path.display().to_string(),
            gpx_path: gpx_path.display().to_string(),
            ride,
        }
    }

    fn entries(&self) -> Vec<RideEntry> {
        self.load().rides.into_iter().map(|ride| self.entry(ride)).collect()
    }

    pub fn view(&self, is_default: bool) -> IndexView {
        let _guard = lock();
        IndexView { folder: self.root.display().to_string(), is_default, rides: self.entries() }
    }

    /// Land one pulled ride durably. Idempotent on its `(serial, store_id, id)` key.
    ///
    /// It returns only after the ride object, the GPX and the index have each been fsynced. The
    /// caller may ack after this resolves and at no earlier point.
    pub fn import(&self, req: &ImportRequest) -> Result<Imported, String> {
        let _guard = lock();
        if req.serial.is_empty() {
            return Err("this device reports no serial number, so a ride from it cannot be keyed".into());
        }
        validate_store_id(&req.store_id)?;
        if req.object_id == 0 {
            return Err("objectId zero names no object".into());
        }
        if req.object.is_empty() {
            return Err("that ride object is empty".into());
        }
        if req.object.len() > MAX_RIDE_BYTES {
            return Err(format!("that ride object is {} bytes; the limit is {MAX_RIDE_BYTES}", req.object.len()));
        }
        if req.gpx.is_empty() {
            return Err("that ride converted to an empty GPX".into());
        }
        if req.gpx.len() > MAX_GPX_BYTES {
            return Err(format!("that GPX is {} bytes; the limit is {MAX_GPX_BYTES}", req.gpx.len()));
        }
        if req.track.len() > MAX_TRACK_POINTS {
            return Err(format!("a preview track carries at most {MAX_TRACK_POINTS} points"));
        }

        std::fs::create_dir_all(&self.root).map_err(|e| format!("create {}: {e}", self.root.display()))?;
        std::fs::create_dir_all(&self.archive).map_err(|e| format!("create {}: {e}", self.archive.display()))?;
        let mut index = self.load();
        let key = ride_key(&req.serial, &req.store_id, req.object_id);

        // The idempotent path: everything is already here, so nothing is written or re-stamped.
        if let Some(existing) = index.rides.iter().find(|r| r.key == key) {
            let entry = self.entry(existing.clone());
            if entry.present && entry.gpx_present {
                return Ok(Imported { ride: entry, imported: false });
            }
        }

        let existing = index.rides.iter().position(|r| r.key == key);
        let (ride_file, gpx_file, imported_at) = match existing {
            // A repair: the record survived and a file did not. Keep the names and the original
            // `imported_at`, because this is the same ride arriving again.
            Some(at) => {
                let r = &index.rides[at];
                (r.ride_file.clone(), r.gpx_file.clone(), r.imported_at)
            }
            None => {
                let stem = unique_stem(&index, &self.root, &self.archive, &key, &stem_for(req))?;
                (format!("{stem}.{RIDE_EXT}"), format!("{stem}.{GPX_EXT}"), now_secs())
            }
        };

        // Order is the contract: the lossless object, then the portable GPX, then the index that
        // claims both. A crash at any point leaves an index that does not name this ride.
        durable_write(&self.archive, &ride_file, &req.object, self.crash_at(CrashPoint::BeforeObjectFsync))
            .map_err(|e| format!("write {}: {e}", self.archive.join(&ride_file).display()))?;
        durable_write(&self.root, &gpx_file, req.gpx.as_bytes(), CrashPoint::None)
            .map_err(|e| format!("write {}: {e}", self.root.join(&gpx_file).display()))?;

        let ride = LibraryRide {
            key: key.clone(),
            serial: req.serial.clone(),
            store_id: req.store_id.clone(),
            object_id: req.object_id,
            name: req.name.clone(),
            start_time: req.start_time,
            distance_m: req.distance_m,
            moving_time_s: req.moving_time_s,
            climb_m: req.climb_m,
            points: req.points,
            bytes: req.object.len() as u64,
            crc32: req.crc32,
            imported_at,
            ride_file,
            gpx_file,
            track: req.track.clone(),
        };

        match existing {
            Some(at) => index.rides[at] = ride.clone(),
            None => index.rides.push(ride.clone()),
        }
        if self.crash == CrashPoint::BeforeIndexCommit {
            return Err(format!("{CRASH_MSG} before the index committed"));
        }
        self.commit(&index)?;
        Ok(Imported { ride: self.entry(ride), imported: existing.is_none() })
    }

    /// The stored ride object of one key, which is what a re-export reads.
    pub fn read_object(&self, key: &str) -> Result<Vec<u8>, String> {
        let _guard = lock();
        let ride = self.find(key)?;
        std::fs::read(self.archive.join(&ride.ride_file))
            .map_err(|e| format!("read {}: {e}", self.archive.join(&ride.ride_file).display()))
    }

    /// Write one ride's GPX durably into the visible folder: the repair for a GPX somebody deleted
    /// or renamed. The archived object is the source, so it can always be run again.
    pub fn write_gpx(&self, key: &str, gpx: &str) -> Result<String, String> {
        let _guard = lock();
        if gpx.is_empty() || gpx.len() > MAX_GPX_BYTES {
            return Err(format!("a GPX of {} bytes is outside 1..={MAX_GPX_BYTES}", gpx.len()));
        }
        let ride = self.find(key)?;
        std::fs::create_dir_all(&self.root).map_err(|e| format!("create {}: {e}", self.root.display()))?;
        durable_write(&self.root, &ride.gpx_file, gpx.as_bytes(), CrashPoint::None)
            .map_err(|e| format!("write {}: {e}", self.root.join(&ride.gpx_file).display()))?;
        Ok(self.root.join(&ride.gpx_file).display().to_string())
    }

    fn find(&self, key: &str) -> Result<LibraryRide, String> {
        self.load().rides.into_iter().find(|r| r.key == key).ok_or_else(|| format!("no ride {key} in this library"))
    }

    /// Rewrite the index durably, through its own temp and rename, so a crash mid-write leaves the
    /// previous index whole and loses only the last ride's record.
    fn commit(&self, index: &Index) -> Result<(), String> {
        let body = serde_json::to_vec_pretty(index).map_err(|e| format!("encode {INDEX_FILE}: {e}"))?;
        durable_write(&self.archive, INDEX_FILE, &body, CrashPoint::None)
            .map_err(|e| format!("write {}: {e}", self.archive.join(INDEX_FILE).display()))
    }

    fn crash_at(&self, point: CrashPoint) -> CrashPoint {
        if self.crash == point {
            point
        } else {
            CrashPoint::None
        }
    }
}

/// Read and parse an index file. `None` means missing or unreadable, and the caller decides what
/// that means.
fn read_index(path: &Path) -> Option<Index> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice::<Index>(&bytes) {
        Ok(index) if index.version == INDEX_VERSION => Some(index),
        Ok(_) => None,
        Err(e) => {
            eprintln!("ride library: {} is unreadable ({e}); treating it as empty", path.display());
            None
        }
    }
}

/// Move one file into `dir/name` so that no point of interruption loses it: `rename` on the same
/// volume, otherwise copy, fsync, atomic rename into place, and only then unlink the source. The
/// copy path makes a cross-filesystem move as safe as a same-volume one.
fn move_file_durably(source: &Path, dir: &Path, name: &str) -> Result<(), String> {
    if std::fs::rename(source, dir.join(name)).is_ok() {
        return sync_dir(dir).map_err(|e| format!("sync {}: {e}", dir.display()));
    }
    let bytes = std::fs::read(source).map_err(|e| format!("read {}: {e}", source.display()))?;
    durable_write(dir, name, &bytes, CrashPoint::None)
        .map_err(|e| format!("write {}: {e}", dir.join(name).display()))?;
    std::fs::remove_file(source).map_err(|e| format!("remove {}: {e}", source.display()))
}

/// Shared with `lib/device/rides.ts`: full StoreId hex and decimal u64 ObjectId.
pub fn ride_key(serial: &str, store_id: &str, object_id: u64) -> String {
    format!("{serial}:{store_id}:{object_id}")
}

fn validate_store_id(value: &str) -> Result<(), String> {
    if value.len() == 32 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        Ok(())
    } else {
        Err("storeId must contain exactly 32 lowercase hexadecimal digits".into())
    }
}

fn deserialize_store_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    validate_store_id(&value).map_err(serde::de::Error::custom)?;
    Ok(value)
}

fn serialize_object_id<S: serde::Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_str(value)
}

fn deserialize_object_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let text = String::deserialize(deserializer)?;
    let value = text.parse::<u64>().map_err(serde::de::Error::custom)?;
    if value == 0 || text != value.to_string() {
        return Err(serde::de::Error::custom("objectId must be a canonical nonzero decimal u64 string"));
    }
    Ok(value)
}

/// Write `bytes` to `dir/name` so that a power cut cannot leave a half-file behind.
///
/// Four steps, none of them optional. The write goes into a `.part` sibling, so the destination
/// keeps its previous contents for the whole write. `sync_all` puts the bytes and the inode on the
/// disk; on macOS Rust's std implements it as `F_FULLFSYNC`, which also flushes the drive's own
/// write cache, where plain `fsync` does not. The `rename` is atomic on every filesystem this app
/// runs on, so a reader sees the old file or the new one. The directory fsync makes that rename
/// durable; skipping it is the classic failure where the data survives and the name does not.
///
/// A real IO failure cleans its `.part` up. A [`CrashPoint`] does not, because the point of
/// simulating a power cut is to leave the mess a power cut leaves.
fn durable_write(dir: &Path, name: &str, bytes: &[u8], crash: CrashPoint) -> io::Result<()> {
    let tmp = dir.join(format!(".{name}.part"));
    let result = write_and_sync(&tmp, bytes, crash);
    if result.is_err() {
        if crash == CrashPoint::None {
            let _ = std::fs::remove_file(&tmp);
        }
        return result;
    }
    std::fs::rename(&tmp, dir.join(name))?;
    sync_dir(dir)
}

fn write_and_sync(tmp: &Path, bytes: &[u8], crash: CrashPoint) -> io::Result<()> {
    let mut file = File::create(tmp)?;
    file.write_all(bytes)?;
    if crash != CrashPoint::None {
        // The process dies here. The bytes are in the page cache, the rename never happened, and
        // nothing above this line told anyone the ride was safe.
        return Err(io::Error::new(io::ErrorKind::Interrupted, format!("{CRASH_MSG} before fsync")));
    }
    file.sync_all()
}

/// fsync the directory entry, so the rename itself survives a power cut.
///
/// Unix opens the directory read-only and fsyncs the handle. Windows has no equivalent and does not
/// need one: `File::open` on a directory fails without `FILE_FLAG_BACKUP_SEMANTICS`, and NTFS
/// records a rename in its metadata log, which is committed before `MoveFileEx` returns, so the
/// file's own flush is the whole requirement there.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// `YYYY-MM-DD-some-ride-name`, or just the name when the device had no clock.
///
/// The date leads because a folder sorts by name and rides are read in order. It is UTC, matching
/// the ride object's `start_time`: rendering it locally would file a late-evening ride under the
/// wrong day for anyone west of Greenwich.
fn stem_for(req: &ImportRequest) -> String {
    let name = crate::paths::sanitize_basename(&req.name, "", "ride");
    let mut stem = match utc_date(req.start_time) {
        Some(date) => format!("{date}-{name}"),
        None => name,
    };
    if stem.len() > MAX_STEM {
        // On a char boundary: a stem is a filename, and half a UTF-8 sequence is not one.
        let cut = (0..=MAX_STEM).rev().find(|&n| stem.is_char_boundary(n)).unwrap_or(0);
        stem.truncate(cut);
    }
    stem.trim().trim_end_matches(['.', '-']).to_string()
}

/// A stem unclaimed by the index or either folder, including files from an unreadable index.
fn unique_stem(index: &Index, root: &Path, archive: &Path, key: &str, base: &str) -> Result<String, String> {
    let taken = |candidate: &str| {
        let ride_file = format!("{candidate}.{RIDE_EXT}");
        let gpx_file = format!("{candidate}.{GPX_EXT}");
        index.rides.iter().any(|r| r.ride_file == ride_file || r.gpx_file == gpx_file)
            || archive.join(ride_file).exists()
            || root.join(gpx_file).exists()
    };
    let base = if base.is_empty() { "ride" } else { base };
    if !taken(base) {
        return Ok(base.to_string());
    }
    let with_id = format!("{base}-{}", key.rsplit(':').next().unwrap_or("0"));
    if !taken(&with_id) {
        return Ok(with_id);
    }
    (2..10_000)
        .map(|n| format!("{with_id}-{n}"))
        .find(|candidate| !taken(candidate))
        .ok_or_else(|| "no unused ride filename is available".into())
}

/// `YYYY-MM-DD` in UTC, or `None` for a device whose clock was never set.
///
/// Civil-from-days arithmetic, because a date is fifteen lines and a date library is a dependency.
/// Valid for every timestamp a `u32` can hold.
fn utc_date(start_time: u32) -> Option<String> {
    if start_time == 0 {
        return None;
    }
    let days = (start_time / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Serialize, Deserialize)]
struct Location {
    dir: String,
}

/// The folder the user relocated the library to, if they did.
///
/// A folder that no longer exists is not replaced by the default: the app would then start a second
/// library on an unplugged external drive and report nothing missing. The caller surfaces it.
pub fn configured(config_dir: &Path) -> Option<PathBuf> {
    let bytes = std::fs::read(config_dir.join(LOCATION_FILE)).ok()?;
    let location: Location = serde_json::from_slice(&bytes).ok()?;
    let dir = PathBuf::from(location.dir);
    (!dir.as_os_str().is_empty()).then_some(dir)
}

/// Remember a relocated library. Durable, because forgetting where the rides went is its own kind
/// of data loss.
pub fn remember(config_dir: &Path, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(config_dir).map_err(|e| format!("create {}: {e}", config_dir.display()))?;
    let body = serde_json::to_vec_pretty(&Location { dir: dir.display().to_string() })
        .map_err(|e| format!("encode {LOCATION_FILE}: {e}"))?;
    durable_write(config_dir, LOCATION_FILE, &body, CrashPoint::None)
        .map_err(|e| format!("write {}: {e}", config_dir.join(LOCATION_FILE).display()))
}

/// Move the visible library, the GPX files, to a new folder.
///
/// The internal archive stays in app data. Existing destination files are never replaced.
/// Cross-filesystem moves copy and flush before unlinking the source, and the caller updates the
/// configured location only after success.
pub fn relocate(from: &Path, to: &Path) -> Result<(), String> {
    let _guard = lock();
    if from == to {
        return Ok(());
    }
    if to.starts_with(from) || from.starts_with(to) {
        return Err("pick a folder that is not inside the current one".into());
    }
    std::fs::create_dir_all(to).map_err(|e| format!("create {}: {e}", to.display()))?;
    if !from.exists() {
        return Ok(());
    }

    let entries = std::fs::read_dir(from).map_err(|e| format!("read {}: {e}", from.display()))?;
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter(|name| !name.starts_with('.') && Path::new(name).extension().is_some_and(|e| e == GPX_EXT))
        .collect();
    names.sort();

    // Two passes: the first discovers a collision before anything has moved, and the second
    // re-checks per file, because `rename` clobbers and std has no portable no-clobber rename. The
    // re-check narrows the race to the one rename; a file another program drops into that window
    // can still be lost. The library lock rules out this process racing itself.
    for name in &names {
        if to.join(name).exists() {
            return Err(format!("{} already contains a file named {name} — pick another folder", to.display()));
        }
    }
    for name in &names {
        if to.join(name).exists() {
            return Err(format!("{} now contains a file named {name} — nothing further was moved", to.display()));
        }
        move_file_durably(&from.join(name), to, name)?;
    }
    sync_dir(to).map_err(|e| format!("sync {}: {e}", to.display()))?;
    let _ = sync_dir(from);
    // Best effort: an empty folder left behind is untidy and a failed move is not. A folder that
    // still holds the rider's other files stays.
    let _ = std::fs::remove_dir(from);
    Ok(())
}

#[cfg(test)]
mod tests {
    /// The ack set, as a test helper: which rides are durably on this disk. It is a filter over
    /// `view()`'s entries, and the tests below are about that property.
    fn durable_ids(lib: &Library, serial: &str, store_id: u128) -> Vec<u64> {
        let mut ids: Vec<u64> = lib
            .view(false)
            .rides
            .into_iter()
            .filter(|e| e.present && e.ride.serial == serial && e.ride.store_id == format!("{store_id:032x}"))
            .map(|e| e.ride.object_id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "obc-rides-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// A library over a visible folder and an internal archive: the production shape, in a
    /// sandbox.
    fn library(base: &Path) -> Library {
        Library::new(base.join("rides"), base.join("archive"))
    }

    fn request(serial: &str, store_id: u128, id: u64, name: &str) -> ImportRequest {
        ImportRequest {
            serial: serial.into(),
            store_id: format!("{store_id:032x}"),
            object_id: id,
            name: name.into(),
            start_time: 1_764_547_200, // 2025-12-01
            distance_m: 42_195,
            moving_time_s: 7_200,
            climb_m: 640,
            points: 4_211,
            crc32: 0xdead_beef,
            track: vec![[48.0, 7.85], [48.01, 7.86]],
            object: format!("ride-object-{name}-{id}").into_bytes(),
            gpx: format!("<gpx><trk><name>{name}</name></trk></gpx>"),
        }
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| entries.flatten().filter_map(|e| e.file_name().to_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn a_second_import_of_the_same_ride_writes_nothing_and_re_stamps_nothing() {
        let base = temp("idem");
        let lib = library(&base);
        let req = request("OBC-24-000317", 0xa1b2c3d4, 7, "Dawn Patrol");

        let first = lib.import(&req).expect("first import");
        assert!(first.imported, "the first pull lands the ride");
        let stamp = first.ride.imported_at;
        let archive = base.join("archive");
        let ride_mtime = std::fs::metadata(archive.join(&first.ride.ride_file)).unwrap().modified().unwrap();

        let second = lib.import(&req).expect("second import");
        assert!(!second.imported, "the second pull is a no-op, not a duplicate");
        assert_eq!(second.ride.imported_at, stamp, "imported_at is first-import-wins, like synced_at");
        assert_eq!(second.ride.key, first.ride.key);
        assert_eq!(
            std::fs::metadata(archive.join(&first.ride.ride_file)).unwrap().modified().unwrap(),
            ride_mtime,
            "the ride file was not rewritten"
        );
        assert_eq!(lib.load().rides.len(), 1, "one record, not two");
        assert_eq!(durable_ids(&lib, "OBC-24-000317", 0xa1b2c3d4), vec![7]);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The visible folder holds GPX and nothing else; the archive holds the object and the
    /// index.
    #[test]
    fn an_import_leaves_only_gpx_in_the_visible_folder() {
        let base = temp("split");
        let lib = library(&base);
        let landed = lib.import(&request("S", 1, 1, "Split")).expect("import");

        assert_eq!(file_names(&base.join("rides")), vec![landed.ride.gpx_file.clone()]);
        let mut archived = vec![INDEX_FILE.to_string(), landed.ride.ride_file.clone()];
        archived.sort();
        assert_eq!(file_names(&base.join("archive")), archived);
        assert!(landed.ride.ride_path.starts_with(base.join("archive").to_str().unwrap()));
        assert!(landed.ride.gpx_path.starts_with(base.join("rides").to_str().unwrap()));

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The power goes out between `write()` and `fsync()`. The real `import` runs, and what is
    /// checked is what a restart then sees, because that is what the ack list is computed from. The
    /// ride must be absent from it, so the device is never told and the next pull fetches it
    /// again.
    #[test]
    fn a_crash_between_write_and_fsync_leaves_the_ride_unacked() {
        let base = temp("crash");
        let good = request("OBC-24-000317", 7, 1, "Landed");
        let lost = request("OBC-24-000317", 7, 2, "Interrupted");

        library(&base).import(&good).expect("the first ride lands");

        let err = Library::crashing_at(base.join("rides"), base.join("archive"), CrashPoint::BeforeObjectFsync)
            .import(&lost)
            .expect_err("a crash before fsync must not report success");
        assert!(err.contains(CRASH_MSG), "unexpected failure: {err}");

        // Restart: a new Library over the same folders, reading the index a relaunched app would
        // read.
        let restarted = library(&base);
        assert_eq!(
            durable_ids(&restarted, "OBC-24-000317", 7),
            vec![1],
            "only the fsynced ride is ackable; the interrupted one is not"
        );
        assert!(
            !base.join("archive").join(format!("2025-12-01-Interrupted.{RIDE_EXT}")).exists(),
            "nothing was committed under the destination name"
        );
        assert_eq!(restarted.load().rides.len(), 1, "the index never learned about the lost ride");

        // The next pull, with the power on, lands it and only then makes it ackable.
        library(&base).import(&lost).expect("the retry lands");
        assert_eq!(durable_ids(&library(&base), "OBC-24-000317", 7), vec![1, 2]);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The other half of the ordering: the files are durable and the index never committed. The
    /// ride is still not ackable, and the retry repairs the record without minting a second one.
    #[test]
    fn a_crash_before_the_index_commits_also_leaves_the_ride_unacked() {
        let base = temp("crash-index");
        let req = request("OBC-24-000317", 7, 3, "Half landed");

        Library::crashing_at(base.join("rides"), base.join("archive"), CrashPoint::BeforeIndexCommit)
            .import(&req)
            .expect_err("crash");
        let restarted = library(&base);
        assert!(durable_ids(&restarted, "OBC-24-000317", 7).is_empty(), "an uncommitted index acks nothing");
        assert!(base.join("archive").join(format!("2025-12-01-Half landed.{RIDE_EXT}")).exists(), "the bytes did land");

        let retry = restarted.import(&req).expect("the retry commits");
        assert!(retry.imported);
        assert_eq!(restarted.load().rides.len(), 1, "the retry did not mint a second record");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Card identities that share their first 32 bits must keep recycled object ids distinct.
    #[test]
    fn a_card_change_with_a_recycled_id_keeps_both_rides() {
        let base = temp("store_id");
        let lib = library(&base);
        let serial = "OBC-24-000317";

        let old =
            lib.import(&request(serial, 0xa1b2c3d4_00000000_00000000_00000000, 1, "Old era ride")).expect("old era");
        let new =
            lib.import(&request(serial, 0xa1b2c3d4_00000000_00000000_00000001, 1, "New era ride")).expect("new era");

        assert!(old.imported && new.imported, "the recycled id is a different ride, not a duplicate");
        assert_ne!(old.ride.key, new.ride.key);
        assert_ne!(old.ride.ride_file, new.ride.ride_file, "and it gets its own file");
        assert_eq!(lib.load().rides.len(), 2);

        // Each era acks only its own ids. The old era's record is archival: it names a ride the
        // device no longer has, and nothing in the new era may claim it.
        assert_eq!(durable_ids(&lib, serial, 0xa1b2c3d4_00000000_00000000_00000000), vec![1]);
        assert_eq!(durable_ids(&lib, serial, 0xa1b2c3d4_00000000_00000000_00000001), vec![1]);
        // A different device with the same id is a third ride again.
        assert!(durable_ids(&lib, "OBC-24-000999", 0xa1b2c3d4_00000000_00000000_00000001).is_empty());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// What the device is told matches what is on the disk, including after something deletes an
    /// archive file, which `present` must notice.
    #[test]
    fn durable_ids_follow_the_filesystem_not_the_index() {
        let base = temp("present");
        let lib = library(&base);
        let serial = "OBC-24-000317";
        for id in [4u64, 5, 6] {
            lib.import(&request(serial, 9, id, &format!("Ride {id}"))).expect("import");
        }
        assert_eq!(durable_ids(&lib, serial, 9), vec![4, 5, 6]);

        let gone = lib.load().rides.iter().find(|r| r.object_id == 5).expect("ride 5").ride_file.clone();
        std::fs::remove_file(base.join("archive").join(&gone)).expect("delete the archive file");

        assert_eq!(durable_ids(&lib, serial, 9), vec![4, 6], "a deleted ride is not durable and is not acked");
        let listed = lib.entries();
        assert_eq!(listed.len(), 3, "the record survives so the UI can say the file is missing");
        assert!(!listed.iter().find(|r| r.object_id == 5).unwrap().present);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_missing_gpx_is_repaired_without_re_downloading_the_ride() {
        let base = temp("regpx");
        let lib = library(&base);
        let landed = lib.import(&request("S", 1, 8, "Export me")).expect("import");
        std::fs::remove_file(base.join("rides").join(&landed.ride.gpx_file)).expect("delete the gpx");
        assert!(!lib.entries()[0].gpx_present);

        let object = lib.read_object(&landed.ride.key).expect("the archive is still there");
        assert_eq!(object, request("S", 1, 8, "Export me").object);
        lib.write_gpx(&landed.ride.key, "<gpx>rebuilt</gpx>").expect("re-export");
        assert!(lib.entries()[0].gpx_present);
        assert_eq!(
            std::fs::read_to_string(base.join("rides").join(&landed.ride.gpx_file)).unwrap(),
            "<gpx>rebuilt</gpx>"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_corrupt_index_reads_as_empty_rather_than_as_nonsense() {
        let base = temp("corrupt");
        let lib = library(&base);
        lib.import(&request("S", 1, 1, "A ride")).expect("import");
        std::fs::write(base.join("archive").join(INDEX_FILE), b"{ this is not json").expect("corrupt it");

        assert!(lib.load().rides.is_empty(), "an unreadable index is an empty library");
        assert!(durable_ids(&lib, "S", 1).is_empty(), "and acks nothing — the safe direction");
        // The files are still there, so the next pull re-imports over them.
        assert!(lib.import(&request("S", 1, 1, "A ride")).expect("re-import").imported);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn two_rides_that_want_the_same_filename_get_different_ones() {
        let base = temp("stems");
        let lib = library(&base);
        let a = lib.import(&request("S", 1, 11, "Commute")).expect("a");
        let b = lib.import(&request("S", 1, 12, "Commute")).expect("b");
        assert_ne!(a.ride.ride_file, b.ride.ride_file);
        assert_eq!(a.ride.ride_file, format!("2025-12-01-Commute.{RIDE_EXT}"));
        assert_eq!(b.ride.ride_file, format!("2025-12-01-Commute-12.{RIDE_EXT}"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_ride_name_can_never_name_a_place() {
        let req = request("S", 1, 1, "../../.ssh/authorized_keys");
        let stem = stem_for(&req);
        assert!(!stem.contains('/') && !stem.contains('\\') && !stem.contains(".."), "{stem}");
        assert_eq!(stem, "2025-12-01-authorized_keys");
        assert_eq!(stem_for(&request("S", 1, 1, "")), "2025-12-01-ride");
    }

    #[test]
    fn dates_are_utc_and_absent_when_the_device_had_no_clock() {
        assert_eq!(utc_date(0), None);
        assert_eq!(utc_date(1).as_deref(), Some("1970-01-01"));
        assert_eq!(utc_date(1_764_547_200).as_deref(), Some("2025-12-01"));
        assert_eq!(utc_date(1_767_225_599).as_deref(), Some("2025-12-31"));
        assert_eq!(utc_date(1_767_225_600).as_deref(), Some("2026-01-01"));
        // A leap day, and the last instant a u32 can hold.
        assert_eq!(utc_date(1_709_164_800).as_deref(), Some("2024-02-29"));
        assert_eq!(utc_date(u32::MAX).as_deref(), Some("2106-02-07"));
    }

    #[test]
    fn the_key_uses_full_store_hex_and_decimal_u64() {
        let store = "a1b2c3d4000000000000000000000000";
        for id in [65536, 9007199254740993, u64::MAX] {
            assert_eq!(ride_key("OBC-24-000317", store, id), format!("OBC-24-000317:{store}:{id}"));
        }
        assert_eq!(ride_key("a:b", store, 2), "a:b:a1b2c3d4000000000000000000000000:2");
    }

    fn json_request() -> serde_json::Value {
        serde_json::json!({
            "serial": "OBC-24-000317",
            "storeId": "a1b2c3d4000000000000000000000000",
            "objectId": "65536",
            "name": "Same name",
            "startTime": 1764547200_u32,
            "distanceM": 42195,
            "movingTimeS": 7200,
            "climbM": 640,
            "points": 1,
            "crc32": 0,
            "track": [],
            "object": [1, 2, 3],
            "gpx": "<gpx/>"
        })
    }

    #[test]
    fn json_identity_survives_import_reload_and_duplicate_detection() {
        let base = temp("full-identity");
        for store in ["a1b2c3d4000000000000000000000000", "a1b2c3d4000000000000000000000001"] {
            for id in ["65536", "9007199254740993", "18446744073709551615"] {
                let mut json = json_request();
                json["storeId"] = store.into();
                json["objectId"] = id.into();
                let req: ImportRequest = serde_json::from_value(json).expect("JSON request");
                let landed = library(&base).import(&req).expect("import");
                assert!(landed.imported);
                assert_eq!(landed.ride.key, format!("OBC-24-000317:{store}:{id}"));
                let reply = serde_json::to_value(&landed).expect("JSON response");
                assert_eq!(reply["ride"]["storeId"], store);
                assert_eq!(reply["ride"]["objectId"], id);
                assert!(!library(&base).import(&req).expect("reload and repeat").imported);
            }
        }
        let rides = library(&base).view(false).rides;
        assert_eq!(rides.len(), 6);
        assert!(rides.iter().all(|r| r.present && r.gpx_present));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn malformed_json_identity_is_refused_before_file_creation() {
        let base = temp("invalid-identity");
        for field in ["storeId", "objectId"] {
            let invalid = if field == "storeId" {
                vec![
                    serde_json::json!(7),
                    serde_json::json!(null),
                    "".into(),
                    "a1b2c3d4".into(),
                    "A1B2C3D4000000000000000000000000".into(),
                    "g1b2c3d4000000000000000000000000".into(),
                ]
            } else {
                vec![
                    serde_json::json!(65536),
                    serde_json::json!(null),
                    "".into(),
                    "0".into(),
                    "-1".into(),
                    "+1".into(),
                    "01".into(),
                    "1.0".into(),
                    "1e3".into(),
                    "18446744073709551616".into(),
                ]
            };
            for value in invalid {
                let mut json = json_request();
                json[field] = value;
                assert!(serde_json::from_value::<ImportRequest>(json).is_err(), "invalid {field}");
            }
        }
        let mut req = request("S", 1, 1, "Bad card");
        req.store_id = "invalid".into();
        assert!(library(&base).import(&req).is_err());
        assert!(file_names(&base).is_empty());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn an_old_index_never_overwrites_archived_objects_or_gpx() {
        let base = temp("old-index");
        let req = request("S", 1, 1, "Same name");
        let old = library(&base).import(&req).unwrap();
        let object_path = PathBuf::from(&old.ride.ride_path);
        let gpx_path = PathBuf::from(&old.ride.gpx_path);
        let mut index: serde_json::Value =
            serde_json::from_slice(&std::fs::read(base.join("archive").join(INDEX_FILE)).unwrap()).unwrap();
        index["version"] = 1.into();
        index["rides"][0].as_object_mut().unwrap().remove("storeId");
        index["rides"][0]["epoch"] = 1.into();
        index["rides"][0]["objectId"] = 1.into();
        std::fs::write(base.join("archive").join(INDEX_FILE), serde_json::to_vec(&index).unwrap()).unwrap();
        assert!(library(&base).view(false).rides.is_empty());
        let mut new_req = req.clone();
        new_req.object = b"different ride".to_vec();
        new_req.gpx = "<gpx>different ride</gpx>".into();
        let new = library(&base).import(&new_req).unwrap();
        assert!(new.imported);
        assert_ne!(new.ride.ride_path, old.ride.ride_path);
        assert_ne!(new.ride.gpx_path, old.ride.gpx_path);
        assert_eq!(std::fs::read(object_path).unwrap(), req.object);
        assert_eq!(std::fs::read_to_string(gpx_path).unwrap(), req.gpx);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn oversized_and_empty_imports_are_refused_before_anything_is_written() {
        let base = temp("limits");
        let lib = library(&base);
        let mut req = request("S", 1, 1, "Huge");
        req.object = vec![0; MAX_RIDE_BYTES + 1];
        assert!(lib.import(&req).is_err());
        let mut req = request("S", 1, 1, "Empty");
        req.object.clear();
        assert!(lib.import(&req).is_err());
        let mut req = request("", 1, 1, "No serial");
        req.serial.clear();
        assert!(lib.import(&req).is_err(), "a device with no serial cannot key a ride");
        assert!(lib.load().rides.is_empty(), "nothing was written");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// `present` is not bare existence: a truncated archive file must read as not durable, drop out
    /// of the ack list, and be repaired by the next pull.
    #[test]
    fn a_truncated_archive_is_not_durable_and_is_repaired() {
        let base = temp("truncated");
        let lib = library(&base);
        let landed = lib.import(&request("S", 1, 4, "Torn")).expect("import");
        assert_eq!(durable_ids(&lib, "S", 1), vec![4]);

        let path = base.join("archive").join(&landed.ride.ride_file);
        let whole = std::fs::read(&path).expect("read");
        std::fs::write(&path, &whole[..whole.len() / 2]).expect("truncate");

        assert!(durable_ids(&lib, "S", 1).is_empty(), "a torn file acks nothing");
        assert!(!lib.entries()[0].present, "…and the UI sees it as missing");

        let repaired = lib.import(&request("S", 1, 4, "Torn")).expect("the next pull repairs it");
        assert!(!repaired.imported, "the same ride arriving again, not a new one");
        assert_eq!(durable_ids(&lib, "S", 1), vec![4]);
        assert_eq!(std::fs::read(&path).expect("whole again"), whole);

        let _ = std::fs::remove_dir_all(&base);
    }

    // Relocation.

    #[test]
    fn relocating_moves_the_gpx_files_and_only_them() {
        let base = temp("move");
        let from = base.join("old");
        let to = base.join("new");

        let lib = Library::new(from.clone(), base.join("archive"));
        let landed = lib.import(&request("S", 1, 1, "Travelling")).expect("import");

        assert!(relocate(&from, &from.join("inside")).is_err(), "a move into itself is not a move");

        relocate(&from, &to).expect("relocate");
        let moved = Library::new(to.clone(), base.join("archive"));
        assert_eq!(durable_ids(&moved, "S", 1), vec![1], "the archive did not move, so nothing was lost");
        assert!(to.join(&landed.ride.gpx_file).is_file());
        assert!(moved.entries()[0].gpx_present);
        assert!(!from.exists(), "the old folder is gone once it is empty");

        // Moving into a folder the app has never used works, and a second relocate from an already
        // moved source is a no-op rather than an error.
        relocate(&from, &base.join("third")).expect("nothing to move");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn relocating_refuses_to_overwrite_a_same_named_file() {
        let base = temp("move-collide");
        let from = base.join("old");
        let to = base.join("new");
        let lib = Library::new(from.clone(), base.join("archive"));
        let landed = lib.import(&request("S", 1, 1, "Collides")).expect("import");
        std::fs::create_dir_all(&to).expect("to");
        std::fs::write(to.join(&landed.ride.gpx_file), b"someone else's file").expect("occupy");

        assert!(relocate(&from, &to).is_err(), "an existing file is never overwritten");
        assert!(from.join(&landed.ride.gpx_file).is_file(), "and the source did not move");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_configured_location_survives_a_restart() {
        let base = temp("location");
        let config = base.join("config");
        assert_eq!(configured(&config), None);
        remember(&config, &base.join("rides")).expect("remember");
        assert_eq!(configured(&config), Some(base.join("rides")));
        let _ = std::fs::remove_dir_all(&base);
    }
}
