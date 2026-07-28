//! The managed ride library (E2, #912): a folder of files, a small index, and the durable write
//! that an `ackRides` is allowed to follow.
//!
//! ## Why this is Rust and not a browser
//!
//! `synced` on the device means **"a durable copy of this ride exists off the device"** — it is what
//! unlocks deleting the ride there, and its `synced_at` stamp is the anchor auto-expiry (#638)
//! counts from (`obc-ble-interface-spec.md` §4.4). Saying it when no durable copy exists loses a
//! rider's ride. The hosted tier therefore never acks at all, because a browser download is a blob
//! URL the user may cancel; this tier acks *after* [`Library::import`] returns, and that is the
//! whole reason [`Library::import`] is careful.
//!
//! ## What "durable" costs here
//!
//! Every write in this module goes through [`durable_write`], which is
//! `write_all` → `sync_all` → `rename` → fsync the **directory**. Three of those four are the parts
//! people skip:
//!
//! * `sync_all` is the file's own data *and* metadata (`fsync`; on macOS Rust's std issues
//!   `F_FULLFSYNC`, which also flushes the drive's own write cache).
//! * The `rename` is what makes a torn write invisible: the reader either sees the previous file or
//!   the whole new one, never a prefix.
//! * The directory fsync is what makes the *rename* durable. Without it the file's bytes survive a
//!   power cut and the directory entry naming them may not — the classic "I fsynced, and the file
//!   was gone" bug. (Windows is the documented exception; see [`sync_dir`].)
//!
//! And the **order** matters as much as the calls. The ride object lands and is fsynced first, the
//! GPX second, and the index last — so a crash anywhere in the middle leaves an index that does not
//! mention the ride, the next pull re-downloads it, and the device was never told anything. The one
//! ordering that would lose data — telling the device before the bytes are safe — is not reachable
//! from here, because [`Library::import`] only returns `Ok` after every fsync above has returned.
//! [`CrashPoint`] exists so that claim is a test rather than this paragraph.
//!
//! ## What is in the folder
//!
//! ```text
//!   <library>/
//!     index.json                        the small index — keys, summaries, preview tracks
//!     2026-07-20-schauinsland.gpx       the ride, as GPX 1.1 (what other software reads)
//!     2026-07-20-schauinsland.obcride   the device's own ride object, verbatim (§7.2)
//! ```
//!
//! Both files, not one, and the reason is that neither is a superset of the other *today*. The GPX
//! is the portable artifact — it is why this is a visible folder and not an app database — but
//! `obc_route::track_to_gpx` deliberately omits `<time>` until the device has a real clock, so a
//! GPX-only library would silently discard every point's timestamp. The `.obcride` is the device's
//! bytes byte-for-byte, the ones its whole-object CRC-32 covered, so the library stays lossless and
//! a better exporter can be re-run over an old ride. It costs about a fifth of the GPX's size.
//!
//! ## Identity is `(serial, epoch, id)`
//!
//! A ride's key is the device serial, the device's **store epoch**, and the object id — never the
//! bare id. Ids are recycled after an epoch bump (a reformatted card, a factory reset, a torn
//! id-marks line), so a bare-id library silently discards a new ride that reused an old id. This is
//! the same key the iOS companion uses (`LibraryScope` / `LibraryScopingE2ETests`), deliberately, so
//! the two libraries mean the same thing by "the same ride".

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The index's filename. Visible and readable on purpose: this is a folder a person owns.
pub const INDEX_FILE: &str = "index.json";
/// Bumped only when an older index can no longer be read. An unreadable index is not fatal (see
/// [`Library::load`]) — it re-imports, it never deletes.
const INDEX_VERSION: u32 = 1;
/// The stored ride object's extension. Not `.ride`: the point of the suffix is that it is obviously
/// ours and obviously not a GPX.
const RIDE_EXT: &str = "obcride";
const GPX_EXT: &str = "gpx";

/// Largest ride object this command will accept, and it is a ceiling rather than an expectation:
/// the §7.2 object is `31 + name + 18 × points`, so 16 MB is roughly 900 000 points — ten days of
/// continuous 1 Hz recording. It exists because a Tauri command is a door, not because anything
/// legitimate approaches it.
const MAX_RIDE_BYTES: usize = 16 * 1024 * 1024;
/// The GPX is the same track as text — five to six times the object. 128 MB keeps the same margin.
const MAX_GPX_BYTES: usize = 128 * 1024 * 1024;
/// Preview tracks are drawn a few hundred pixels wide; more points than this would be index weight
/// nobody can see. The frontend downsamples to it and this is the enforcement.
const MAX_TRACK_POINTS: usize = 512;
/// Longest filename stem the library will mint, before its extension.
const MAX_STEM: usize = 64;

/// The pointer file that remembers a relocated library. Lives in the app's config directory rather
/// than in the library — a folder that names itself could not be found once it moved.
const LOCATION_FILE: &str = "ride-library.json";

// ============================ the records ============================

/// One ride in the library, as `index.json` **stores** it.
///
/// Only facts that stay true: no absolute paths (the folder can move) and no "is the file there"
/// (a person can delete it in the file manager). Those are [`RideEntry`]'s, recomputed on every
/// read — an index that insisted otherwise would be the app lying about what it has.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRide {
    /// `serial:epoch:objectId` — minted here, never taken from the caller.
    pub key: String,
    pub serial: String,
    pub epoch: u32,
    pub object_id: u16,
    pub name: String,
    /// Ride start, unix seconds UTC. `0` on a device that never had a trusted clock.
    pub start_time: u32,
    pub distance_m: u32,
    pub moving_time_s: u32,
    pub climb_m: u32,
    pub points: u32,
    /// Length of the stored ride object, bytes.
    pub bytes: u64,
    /// The device's whole-object CRC-32 of that object — the transfer's own verdict, kept so the
    /// stored copy can be re-checked without the device.
    pub crc32: u32,
    /// When this app first landed the ride, unix seconds. **Never re-stamped**: a second pull is a
    /// no-op, matching `synced_at`'s first-ack-wins rule on the device.
    pub imported_at: u64,
    pub ride_file: String,
    pub gpx_file: String,
    /// A downsampled `[lat, lon]` track for the list's preview, in degrees. Drawn from the ride's
    /// own points — there is no other source, and a straight line between two waypoints would be a
    /// picture of something that did not happen.
    pub track: Vec<[f64; 2]>,
}

/// One ride as the **UI** reads it: the stored record, plus what only the filesystem can say.
///
/// The two extra pairs are recomputed on every read and never written down. `present` is the one
/// that matters: a ride whose object is gone is not a durable copy, so it is not acked and it is
/// pulled again.
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

/// So an entry reads as the ride it describes (`entry.key`, not `entry.ride.key`). The wrapper adds
/// facts about the filesystem; it is not a different kind of thing.
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

/// What the UI is handed: where the folder is, and what is in it.
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
/// A ride object is hundreds of kilobytes and a JSON number array costs about four bytes of text
/// per byte — which is why maps and firmware images use the raw-body path in [`crate::usb`] and
/// deliberately not why this one does. A ride is two to three orders of magnitude smaller than the
/// case that forced that machinery, and the GPX beside it is text anyway; a structured command with
/// a stated ceiling is the cheaper thing to review.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    pub serial: String,
    pub epoch: u32,
    pub object_id: u16,
    pub name: String,
    pub start_time: u32,
    pub distance_m: u32,
    pub moving_time_s: u32,
    pub climb_m: u32,
    /// Points in the recorded track. From the caller's decode of the object — this crate has no
    /// ride-object decoder and must not grow one: the codecs live once, in `lib/usb/objects.ts`,
    /// pinned to `specs/vectors/`. A second decoder here would be a second thing to drift.
    pub points: u32,
    pub crc32: u32,
    pub track: Vec<[f64; 2]>,
    /// The §7.2 ride object exactly as it came off the wire.
    pub object: Vec<u8>,
    /// The GPX 1.1 document, from the same `obc_route::track_to_gpx` the device runs (through the
    /// wasm bridge). There is no GPX writer in this crate and there must never be one.
    pub gpx: String,
}

/// [`Library::import`]'s answer. `imported` is false when the ride was already in the library — the
/// idempotent case, and the one that must not re-stamp anything.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    pub ride: RideEntry,
    pub imported: bool,
}

/// Where a durable write is interrupted.
///
/// Production constructs a [`Library`] with [`CrashPoint::None`] and there is no way to ask for
/// anything else from outside this module — the other variants exist so "the ack follows the
/// fsync" is checked by running the real [`Library::import`] with the power cut at a chosen
/// instant, rather than by reading this file and believing it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CrashPoint {
    #[default]
    None,
    /// The bytes reached `write()` and the process died before `fsync` — the case that decides
    /// whether this feature can lose a ride.
    BeforeObjectFsync,
    /// The ride's two files are durable and the process died before the index committed.
    BeforeIndexCommit,
}

/// The message a simulated crash returns, so a test can tell it from a real IO error.
const CRASH_MSG: &str = "simulated power loss";

// ============================ the library ============================

pub struct Library {
    root: PathBuf,
    crash: CrashPoint,
}

impl Library {
    pub fn new(root: PathBuf) -> Self {
        Library { root, crash: CrashPoint::None }
    }

    /// A library that dies at `crash`. Test-only by construction, not by convention.
    #[cfg(test)]
    fn crashing_at(root: PathBuf, crash: CrashPoint) -> Self {
        Library { root, crash }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The index as it is on disk right now, with existence recomputed per ride.
    ///
    /// A missing index is an empty library. A **corrupt** index is also an empty library, and that
    /// is the safe direction rather than a shrug: the worst it costs is re-downloading rides the
    /// device still holds (the ack is add-only and `synced_at` is first-ack-wins, so nothing is
    /// disturbed there), while any attempt to salvage a half-parsed index risks writing over the
    /// files it half-understood.
    fn load(&self) -> Index {
        let Ok(bytes) = std::fs::read(self.root.join(INDEX_FILE)) else {
            return Index { version: INDEX_VERSION, rides: Vec::new() };
        };
        let mut index: Index = match serde_json::from_slice(&bytes) {
            Ok(index) => index,
            Err(e) => {
                eprintln!("ride library: {INDEX_FILE} is unreadable ({e}); starting from empty");
                Index { version: INDEX_VERSION, rides: Vec::new() }
            }
        };
        index.version = INDEX_VERSION;
        index
    }

    /// A stored record, joined to the filesystem as it is right now.
    fn entry(&self, ride: LibraryRide) -> RideEntry {
        let ride_path = self.root.join(&ride.ride_file);
        let gpx_path = self.root.join(&ride.gpx_file);
        RideEntry {
            present: ride_path.is_file(),
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
        IndexView { folder: self.root.display().to_string(), is_default, rides: self.entries() }
    }

    /// The ride ids of `(serial, epoch)` whose object is **on disk right now** — the exact set this
    /// app is entitled to ack.
    ///
    /// This is the one function that answers "what is durably here", and the frontend acks its
    /// result rather than the set of rides it thinks it just wrote. That is deliberate: a ride whose
    /// import failed, whose file the user deleted from the file manager, or whose write was
    /// interrupted by a power cut is absent from this list and therefore never flagged on the
    /// device. Re-sending the whole list every pull is also what heals a device that lost its
    /// `/tracks/SYNCED.SET` (§4.4: an ack is add-only, and unknown ids are ignored).
    pub fn durable_ids(&self, serial: &str, epoch: u32) -> Vec<u16> {
        let mut ids: Vec<u16> = self
            .entries()
            .into_iter()
            .filter(|e| e.present && e.ride.serial == serial && e.ride.epoch == epoch)
            .map(|e| e.ride.object_id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Land one pulled ride durably. Idempotent on its `(serial, epoch, id)` key.
    ///
    /// Returns only after the ride object, the GPX and the index have each been fsynced. The caller
    /// may ack **after** this resolves and at no earlier point.
    pub fn import(&self, req: &ImportRequest) -> Result<Imported, String> {
        if req.serial.is_empty() {
            return Err("this device reports no serial number, so a ride from it cannot be keyed".into());
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
        let mut index = self.load();
        let key = ride_key(&req.serial, req.epoch, req.object_id);

        // The idempotent path: everything already here, nothing written, nothing re-stamped.
        if let Some(existing) = index.rides.iter().find(|r| r.key == key) {
            let entry = self.entry(existing.clone());
            if entry.present && entry.gpx_present {
                return Ok(Imported { ride: entry, imported: false });
            }
        }

        let existing = index.rides.iter().position(|r| r.key == key);
        let (ride_file, gpx_file, imported_at) = match existing {
            // A repair: the record survived and a file did not. Keep the names and the original
            // `imported_at` — this is the same ride arriving again, not a new one.
            Some(at) => {
                let r = &index.rides[at];
                (r.ride_file.clone(), r.gpx_file.clone(), r.imported_at)
            }
            None => {
                let stem = unique_stem(&index, &key, &stem_for(req));
                (format!("{stem}.{RIDE_EXT}"), format!("{stem}.{GPX_EXT}"), now_secs())
            }
        };

        // Order is the contract: the lossless object, then the portable GPX, then the index that
        // claims both. A crash at any point leaves an index that does not name this ride.
        durable_write(&self.root, &ride_file, &req.object, self.crash_at(CrashPoint::BeforeObjectFsync))
            .map_err(|e| format!("write {}: {e}", self.root.join(&ride_file).display()))?;
        durable_write(&self.root, &gpx_file, req.gpx.as_bytes(), CrashPoint::None)
            .map_err(|e| format!("write {}: {e}", self.root.join(&gpx_file).display()))?;

        let ride = LibraryRide {
            key: key.clone(),
            serial: req.serial.clone(),
            epoch: req.epoch,
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

    /// The stored ride object of one key — what a re-export reads.
    pub fn read_object(&self, key: &str) -> Result<Vec<u8>, String> {
        let ride = self.find(key)?;
        std::fs::read(self.root.join(&ride.ride_file))
            .map_err(|e| format!("read {}: {e}", self.root.join(&ride.ride_file).display()))
    }

    /// (Re-)write one ride's GPX durably — the per-ride and bulk export, and the repair for a GPX
    /// somebody deleted. The ride object is the source, so this can always be run again.
    pub fn write_gpx(&self, key: &str, gpx: &str) -> Result<String, String> {
        if gpx.is_empty() || gpx.len() > MAX_GPX_BYTES {
            return Err(format!("a GPX of {} bytes is outside 1..={MAX_GPX_BYTES}", gpx.len()));
        }
        let ride = self.find(key)?;
        durable_write(&self.root, &ride.gpx_file, gpx.as_bytes(), CrashPoint::None)
            .map_err(|e| format!("write {}: {e}", self.root.join(&ride.gpx_file).display()))?;
        Ok(self.root.join(&ride.gpx_file).display().to_string())
    }

    fn find(&self, key: &str) -> Result<LibraryRide, String> {
        self.load().rides.into_iter().find(|r| r.key == key).ok_or_else(|| format!("no ride {key} in this library"))
    }

    /// Rewrite the index durably. Its own temp-and-rename, so a crash mid-write leaves the previous
    /// index whole rather than a truncated one — losing the *last* ride's record, never the rest.
    fn commit(&self, index: &Index) -> Result<(), String> {
        let body = serde_json::to_vec_pretty(index).map_err(|e| format!("encode {INDEX_FILE}: {e}"))?;
        durable_write(&self.root, INDEX_FILE, &body, CrashPoint::None)
            .map_err(|e| format!("write {}: {e}", self.root.join(INDEX_FILE).display()))
    }

    fn crash_at(&self, point: CrashPoint) -> CrashPoint {
        if self.crash == point {
            point
        } else {
            CrashPoint::None
        }
    }
}

/// `serial:epoch:objectId`.
///
/// Byte-for-byte the string `lib/device/rides.ts`'s `rideKey()` builds, because the two sides look
/// each other's entries up by it. `frontend_and_backend_agree_on_the_key` in `library.test.ts`
/// pins the pair.
pub fn ride_key(serial: &str, epoch: u32, object_id: u16) -> String {
    format!("{serial}:{epoch}:{object_id}")
}

// ============================ durability ============================

/// Write `bytes` to `dir/name` so that a power cut cannot leave a half-file behind.
///
/// The four steps, and why each one is not optional:
///
/// 1. **into a `.part` sibling** — the destination keeps its previous contents (or its absence)
///    for the whole write;
/// 2. **`sync_all`** — the bytes and the inode reach the disk. On macOS Rust's std implements this
///    as `F_FULLFSYNC`, which also flushes the drive's own write cache; plain `fsync` there does
///    not;
/// 3. **`rename`** — atomic on every filesystem this app runs on: a reader sees old or new;
/// 4. **fsync the directory** — makes step 3 durable. Skipping it is the classic failure where the
///    data survives and the name pointing at it does not.
///
/// A real IO failure cleans its `.part` up. A [`CrashPoint`] deliberately does not — the point of
/// simulating a power cut is to leave exactly the mess a power cut leaves.
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

/// fsync the directory entry — i.e. make the rename itself survive a power cut.
///
/// **Unix** opens the directory read-only and fsyncs the handle, which is the portable way to do it.
///
/// **Windows** has no equivalent and does not need one: `File::open` on a directory fails without
/// `FILE_FLAG_BACKUP_SEMANTICS`, and NTFS records a rename in its metadata log, which is committed
/// before `MoveFileEx` returns. The file's own `FlushFileBuffers` (step 2, which `sync_all` is) is
/// therefore the whole requirement there. This is the "where the platform requires it" clause, and
/// it is a no-op on exactly one platform for a stated reason rather than everywhere for none.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

// ============================ naming ============================

/// `YYYY-MM-DD-some-ride-name`, or just the name when the device had no clock.
///
/// The date leads because a folder sorts by name and rides are read in order; the name follows
/// because the rider chose it. The date is **UTC**, matching the ride object's `start_time` and the
/// hosted tier's `rideFilename()` — rendering it locally would file a late-evening ride under the
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

/// A stem no other key in the index has claimed.
///
/// Deterministic rather than filesystem-probing (`paths::unique_in`'s job for maps): two rides can
/// legitimately share a date and a name, and the disambiguator that means something is the object
/// id — the thing that actually differs.
fn unique_stem(index: &Index, key: &str, base: &str) -> String {
    let taken = |candidate: &str| {
        index.rides.iter().any(|r| r.key != key && (r.ride_file.starts_with(&format!("{candidate}."))))
    };
    let base = if base.is_empty() { "ride".to_string() } else { base.to_string() };
    if !taken(&base) {
        return base;
    }
    let with_id = format!("{base}-{}", key.rsplit(':').next().unwrap_or("0"));
    if !taken(&with_id) {
        return with_id;
    }
    (2..10_000).map(|n| format!("{with_id}-{n}")).find(|candidate| !taken(candidate)).unwrap_or(with_id)
}

/// `YYYY-MM-DD` in UTC, or `None` for a device whose clock was never set (`start_time == 0`).
///
/// Howard Hinnant's civil-from-days, because a date is fifteen lines and a date library is a
/// dependency. Valid for every timestamp a `u32` can hold (1970 → 2106).
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

// ============================ where the folder is ============================

#[derive(Serialize, Deserialize)]
struct Location {
    dir: String,
}

/// The folder the user relocated the library to, if they did.
///
/// A folder that no longer exists is **not** silently replaced by the default: the app would then
/// quietly start a second library on an unplugged external drive and report nothing missing. The
/// caller surfaces it instead.
pub fn configured(config_dir: &Path) -> Option<PathBuf> {
    let bytes = std::fs::read(config_dir.join(LOCATION_FILE)).ok()?;
    let location: Location = serde_json::from_slice(&bytes).ok()?;
    let dir = PathBuf::from(location.dir);
    (!dir.as_os_str().is_empty()).then_some(dir)
}

/// Remember a relocated library. Durable, because forgetting where the rides went is its own
/// data-loss story.
pub fn remember(config_dir: &Path, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(config_dir).map_err(|e| format!("create {}: {e}", config_dir.display()))?;
    let body = serde_json::to_vec_pretty(&Location { dir: dir.display().to_string() })
        .map_err(|e| format!("encode {LOCATION_FILE}: {e}"))?;
    durable_write(config_dir, LOCATION_FILE, &body, CrashPoint::None)
        .map_err(|e| format!("write {}: {e}", config_dir.join(LOCATION_FILE).display()))
}

/// Move an existing library to a new folder, file by file.
///
/// Refuses to merge into a folder that already holds a library: two indexes cannot be reconciled
/// without deciding which of two records for one key wins, and the honest answer is that the user
/// should pick an empty folder. Refuses nesting for the same reason a move into itself is not a
/// move.
///
/// Each file is `rename`d where the filesystem allows it (same volume, instant and atomic) and
/// otherwise copied durably and then unlinked — never unlinked before the copy is fsynced, which is
/// the whole difference between relocating a library and losing one.
///
/// **`index.json` moves last**, which decides what a half-finished move looks like. The caller only
/// re-points the app after this returns `Ok`, so a failure partway leaves the app reading the *old*
/// folder — and the old folder still has its index. The rides that did move read as missing there
/// and are pulled again; the copies already in the new folder are sitting in a directory the rider
/// chose and can see. Untidy, and nothing is deleted or unreachable, which is the only property
/// worth designing for here.
pub fn relocate(from: &Path, to: &Path) -> Result<(), String> {
    if from == to {
        return Ok(());
    }
    if to.starts_with(from) || from.starts_with(to) {
        return Err("pick a folder that is not inside the current one".into());
    }
    std::fs::create_dir_all(to).map_err(|e| format!("create {}: {e}", to.display()))?;
    if to.join(INDEX_FILE).exists() {
        return Err(format!("{} already holds a ride library — pick an empty folder", to.display()));
    }
    if !from.exists() {
        return Ok(());
    }

    let entries = std::fs::read_dir(from).map_err(|e| format!("read {}: {e}", from.display()))?;
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        // A leftover `.part` is the residue of an interrupted write and is named by nothing.
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort_by_key(|name| name == INDEX_FILE);

    for name in &names {
        let path = from.join(name);
        let target = to.join(name);
        if std::fs::rename(&path, &target).is_ok() {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        durable_write(to, name, &bytes, CrashPoint::None).map_err(|e| format!("write {}: {e}", target.display()))?;
        std::fs::remove_file(&path).map_err(|e| format!("remove {}: {e}", path.display()))?;
    }
    sync_dir(to).map_err(|e| format!("sync {}: {e}", to.display()))?;
    // Best effort: an empty folder left behind is untidy, a failed move is not.
    let _ = std::fs::remove_dir(from);
    Ok(())
}

#[cfg(test)]
mod tests {
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

    fn request(serial: &str, epoch: u32, id: u16, name: &str) -> ImportRequest {
        ImportRequest {
            serial: serial.into(),
            epoch,
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

    #[test]
    fn a_second_import_of_the_same_ride_writes_nothing_and_re_stamps_nothing() {
        let root = temp("idem");
        let lib = Library::new(root.clone());
        let req = request("OBC-24-000317", 0xa1b2c3d4, 7, "Dawn Patrol");

        let first = lib.import(&req).expect("first import");
        assert!(first.imported, "the first pull lands the ride");
        let stamp = first.ride.imported_at;
        let ride_mtime = std::fs::metadata(root.join(&first.ride.ride_file)).unwrap().modified().unwrap();

        let second = lib.import(&req).expect("second import");
        assert!(!second.imported, "the second pull is a no-op, not a duplicate");
        assert_eq!(second.ride.imported_at, stamp, "imported_at is first-import-wins, like synced_at");
        assert_eq!(second.ride.key, first.ride.key);
        assert_eq!(
            std::fs::metadata(root.join(&first.ride.ride_file)).unwrap().modified().unwrap(),
            ride_mtime,
            "the ride file was not rewritten"
        );
        assert_eq!(lib.load().rides.len(), 1, "one record, not two");
        assert_eq!(lib.durable_ids("OBC-24-000317", 0xa1b2c3d4), vec![7]);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **Acceptance #2, the one that decides whether this feature can lose a ride.**
    ///
    /// The power goes out between `write()` and `fsync()`. The real `import` runs; what is checked
    /// is what a *restart* then sees, because that is what the ack list is computed from. The ride
    /// must be absent from `durable_ids` — which is the frontend's ack list — so the device is
    /// never told, and the next pull fetches the ride again.
    #[test]
    fn a_crash_between_write_and_fsync_leaves_the_ride_unacked() {
        let root = temp("crash");
        let good = request("OBC-24-000317", 7, 1, "Landed");
        let lost = request("OBC-24-000317", 7, 2, "Interrupted");

        Library::new(root.clone()).import(&good).expect("the first ride lands");

        let err = Library::crashing_at(root.clone(), CrashPoint::BeforeObjectFsync)
            .import(&lost)
            .expect_err("a crash before fsync must not report success");
        assert!(err.contains(CRASH_MSG), "unexpected failure: {err}");

        // Restart: a brand-new Library over the same folder, reading the same index a relaunched
        // app would read.
        let restarted = Library::new(root.clone());
        assert_eq!(
            restarted.durable_ids("OBC-24-000317", 7),
            vec![1],
            "only the fsynced ride is ackable; the interrupted one is not"
        );
        assert!(
            !root.join(format!("2025-12-01-Interrupted.{RIDE_EXT}")).exists(),
            "nothing was committed under the destination name"
        );
        assert_eq!(restarted.load().rides.len(), 1, "the index never learned about the lost ride");

        // …and the next pull, with the power on, lands it and only then makes it ackable.
        Library::new(root.clone()).import(&lost).expect("the retry lands");
        assert_eq!(Library::new(root.clone()).durable_ids("OBC-24-000317", 7), vec![1, 2]);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The other half of the ordering: the files are durable and the *index* never committed. The
    /// ride is still not ackable — the index is what `durable_ids` reads — and the retry repairs
    /// the record without minting a second one.
    #[test]
    fn a_crash_before_the_index_commits_also_leaves_the_ride_unacked() {
        let root = temp("crash-index");
        let req = request("OBC-24-000317", 7, 3, "Half landed");

        Library::crashing_at(root.clone(), CrashPoint::BeforeIndexCommit).import(&req).expect_err("crash");
        let restarted = Library::new(root.clone());
        assert!(restarted.durable_ids("OBC-24-000317", 7).is_empty(), "an uncommitted index acks nothing");
        assert!(root.join(format!("2025-12-01-Half landed.{RIDE_EXT}")).exists(), "the bytes did land");

        let retry = restarted.import(&req).expect("the retry commits");
        assert!(retry.imported);
        assert_eq!(restarted.load().rides.len(), 1, "the retry did not mint a second record");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **Acceptance #3.** A chip erase re-mints the store epoch and the device starts assigning ids
    /// from 1 again, so a *new* ride arrives under an id an *old* ride already used. Both must
    /// survive: bare-id dedupe would silently discard the new one, which is the 2026-07-12 incident
    /// the iOS `LibraryScopingE2ETests` replays.
    #[test]
    fn an_epoch_bump_with_a_recycled_id_keeps_both_rides() {
        let root = temp("epoch");
        let lib = Library::new(root.clone());
        let serial = "OBC-24-000317";

        let old = lib.import(&request(serial, 0x1111_1111, 1, "Old era ride")).expect("old era");
        let new = lib.import(&request(serial, 0x2222_2222, 1, "New era ride")).expect("new era");

        assert!(old.imported && new.imported, "the recycled id is a different ride, not a duplicate");
        assert_ne!(old.ride.key, new.ride.key);
        assert_ne!(old.ride.ride_file, new.ride.ride_file, "and it gets its own file");
        assert_eq!(lib.load().rides.len(), 2);

        // Each era acks only its own ids. The old era's record is archival — it names a ride the
        // device no longer has, and nothing in the new era may claim it.
        assert_eq!(lib.durable_ids(serial, 0x1111_1111), vec![1]);
        assert_eq!(lib.durable_ids(serial, 0x2222_2222), vec![1]);
        // A different device with the same id is a third ride again.
        assert!(lib.durable_ids("OBC-24-000999", 0x2222_2222).is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **Acceptance #4.** What the device is told matches what is on the disk — including after
    /// someone deletes a file in the file manager, which a visible folder invites.
    #[test]
    fn durable_ids_follow_the_filesystem_not_the_index() {
        let root = temp("present");
        let lib = Library::new(root.clone());
        let serial = "OBC-24-000317";
        for id in [4u16, 5, 6] {
            lib.import(&request(serial, 9, id, &format!("Ride {id}"))).expect("import");
        }
        assert_eq!(lib.durable_ids(serial, 9), vec![4, 5, 6]);

        let gone = lib.load().rides.iter().find(|r| r.object_id == 5).expect("ride 5").ride_file.clone();
        std::fs::remove_file(root.join(&gone)).expect("delete it from the folder");

        assert_eq!(lib.durable_ids(serial, 9), vec![4, 6], "a deleted ride is not durable and is not acked");
        let listed = lib.entries();
        assert_eq!(listed.len(), 3, "the record survives so the UI can say the file is missing");
        assert!(!listed.iter().find(|r| r.object_id == 5).unwrap().present);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_gpx_is_repaired_without_re_downloading_the_ride() {
        let root = temp("regpx");
        let lib = Library::new(root.clone());
        let landed = lib.import(&request("S", 1, 8, "Export me")).expect("import");
        std::fs::remove_file(root.join(&landed.ride.gpx_file)).expect("delete the gpx");
        assert!(!lib.entries()[0].gpx_present);

        let object = lib.read_object(&landed.ride.key).expect("the archive is still there");
        assert_eq!(object, request("S", 1, 8, "Export me").object);
        lib.write_gpx(&landed.ride.key, "<gpx>rebuilt</gpx>").expect("re-export");
        assert!(lib.entries()[0].gpx_present);
        assert_eq!(std::fs::read_to_string(root.join(&landed.ride.gpx_file)).unwrap(), "<gpx>rebuilt</gpx>");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_corrupt_index_reads_as_empty_rather_than_as_nonsense() {
        let root = temp("corrupt");
        let lib = Library::new(root.clone());
        lib.import(&request("S", 1, 1, "A ride")).expect("import");
        std::fs::write(root.join(INDEX_FILE), b"{ this is not json").expect("corrupt it");

        assert!(lib.load().rides.is_empty(), "an unreadable index is an empty library");
        assert!(lib.durable_ids("S", 1).is_empty(), "and acks nothing — the safe direction");
        // The files are still there; the next pull re-imports over them.
        assert!(lib.import(&request("S", 1, 1, "A ride")).expect("re-import").imported);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn two_rides_that_want_the_same_filename_get_different_ones() {
        let root = temp("stems");
        let lib = Library::new(root.clone());
        let a = lib.import(&request("S", 1, 11, "Commute")).expect("a");
        let b = lib.import(&request("S", 1, 12, "Commute")).expect("b");
        assert_ne!(a.ride.ride_file, b.ride.ride_file);
        assert_eq!(a.ride.ride_file, format!("2025-12-01-Commute.{RIDE_EXT}"));
        assert_eq!(b.ride.ride_file, format!("2025-12-01-Commute-12.{RIDE_EXT}"));
        let _ = std::fs::remove_dir_all(&root);
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
    fn the_key_is_serial_epoch_id() {
        // The exact string `lib/device/rides.ts`'s `rideKey()` builds — an epoch is a decimal u32
        // on both sides, never hex, or the two libraries would disagree about one ride.
        assert_eq!(ride_key("OBC-24-000317", 0xa1b2c3d4, 7), "OBC-24-000317:2712847316:7");
        assert_eq!(ride_key("", 0, 0), ":0:0");
        // A serial containing the separator still keys unambiguously, because the last two fields
        // are numbers and the split that matters is from the right.
        assert_eq!(ride_key("a:b", 1, 2), "a:b:1:2");
    }

    #[test]
    fn oversized_and_empty_imports_are_refused_before_anything_is_written() {
        let root = temp("limits");
        let lib = Library::new(root.clone());
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
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn relocating_moves_the_library_and_refuses_to_merge_into_another_one() {
        let base = temp("move");
        let from = base.join("old");
        let to = base.join("new");
        let occupied = base.join("occupied");

        let lib = Library::new(from.clone());
        let landed = lib.import(&request("S", 1, 1, "Travelling")).expect("import");
        Library::new(occupied.clone()).import(&request("S", 1, 2, "Already here")).expect("import");

        assert!(relocate(&from, &occupied).is_err(), "two libraries must not be merged silently");
        assert!(relocate(&from, &from.join("inside")).is_err(), "a move into itself is not a move");

        relocate(&from, &to).expect("relocate");
        let moved = Library::new(to.clone());
        assert_eq!(moved.durable_ids("S", 1), vec![1]);
        assert!(to.join(&landed.ride.ride_file).is_file());
        assert!(to.join(&landed.ride.gpx_file).is_file());
        assert!(!from.exists(), "the old folder is gone once it is empty");

        // Moving into a folder the app has never used works, and a second relocate from an empty
        // (already-moved) source is a no-op rather than an error.
        relocate(&from, &base.join("third")).expect("nothing to move");

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
