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
//! ## What is where
//!
//! Two directories, and the split is what each one is *for*:
//!
//! ```text
//!   <library>/                            the folder the rider owns — GPX only
//!     2026-07-20-schauinsland.gpx         the ride, as GPX 1.1 (what other software reads)
//!
//!   <app data>/ride-archive/              internal — the app's own store
//!     index.json                          the small index — keys, summaries, preview tracks
//!     2026-07-20-schauinsland.obcride     the device's own ride object, verbatim (§7.2)
//! ```
//!
//! The visible folder is the product: a folder of GPX files a person can back up, sync, and drag
//! into anything. The `.obcride` archive is the device's bytes byte-for-byte, the ones its
//! whole-object CRC-32 covered — it is what keeps the library lossless while
//! `obc_route::track_to_gpx` still omits `<time>`, and what a better exporter can be re-run over.
//! It is an implementation detail, so it lives in app data with the index rather than cluttering
//! the rider's folder (a `.obcride` next to every GPX invited "what are these, can I delete them" —
//! and deleting one silently un-backed-up a ride the device had been told was safe).
//!
//! Only the **visible** folder is relocatable ("Change…" → [`relocate`]); the archive stays put in
//! app data, because it is the app's own store and a folder that follows another folder around is
//! two ways to lose it. Libraries written by builds before this split (both files plus the index in
//! the one visible folder) are moved over by [`Library::migrate`], durably and idempotently.
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
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// One process-wide lock over every disk-touching library operation.
///
/// Tauri runs commands concurrently, and this module's correctness arguments are all sequential:
/// the migration's "index moves last", the import's "object, GPX, index in that order", and the
/// stem-uniqueness check against the index it just read. Two commands interleaving those steps can
/// commit a truncated `.obcride` through the deterministic `.part` name, or mint a stem a
/// concurrently-migrated file already uses — both of which end in an ack for bytes that are not
/// whole. A single coarse mutex is deliberately the whole answer: every operation here is a few
/// small files, so there is nothing worth being clever about.
static LIBRARY_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    // A panic while holding the lock poisons it; the disk state is still governed by the
    // rename-last discipline, so the next operation may simply proceed.
    LIBRARY_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The index's filename. Readable JSON on purpose, even though it now lives in app data: it is
/// still the record a person (or a support thread) can open and read.
pub const INDEX_FILE: &str = "index.json";
/// Bumped only when an older index can no longer be read. An unreadable index is not fatal (see
/// [`Library::load`]) — it re-imports, it never deletes.
const INDEX_VERSION: u32 = 1;
/// The stored ride object's extension. Not `.ride`: the point of the suffix is that it is obviously
/// ours and obviously not a GPX — which is also what lets [`Library::migrate`] move exactly our
/// files and nothing else out of a pre-split folder.
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
/// Only facts that stay true: no absolute paths (the GPX folder can move) and no "is the file
/// there" (a person can delete a GPX in the file manager). Those are [`RideEntry`]'s, recomputed on
/// every read — an index that insisted otherwise would be the app lying about what it has.
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
    /// Basename of the archived ride object, in the **archive** directory.
    pub ride_file: String,
    /// Basename of the GPX, in the **visible** folder.
    pub gpx_file: String,
    /// A downsampled `[lat, lon]` track for the list's preview, in degrees. Drawn from the ride's
    /// own points — there is no other source, and a straight line between two waypoints would be a
    /// picture of something that did not happen.
    pub track: Vec<[f64; 2]>,
}

/// One ride as the **UI** reads it: the stored record, plus what only the filesystem can say.
///
/// The two extra pairs are recomputed on every read and never written down. `present` is the one
/// that matters — "the archive file exists in the archive directory": a ride whose object is gone
/// is not a durable copy, so it is not acked and it is pulled again. `gpx_present` is about the
/// visible folder, and a missing GPX is only a re-export away (the archive is its source).
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

/// What the UI is handed: where the visible folder is, and what is in the library.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct IndexView {
    pub folder: String,
    /// True when the folder is the app's default rather than one the user picked.
    pub is_default: bool,
    pub rides: Vec<RideEntry>,
    /// Set when [`Library::migrate`] failed on this open — legacy files are still sitting in the
    /// visible folder and the library is reading past them. Filled in by the command layer (which
    /// is the one that ran the migration); surfaced so the failure is a sentence on screen rather
    /// than an `eprintln!` nobody sees.
    pub migration_warning: Option<String>,
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
/// fsync" (and now also "a half-run migration re-runs cleanly") is checked by running the real code
/// with the power cut at a chosen instant, rather than by reading this file and believing it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CrashPoint {
    #[default]
    None,
    /// The bytes reached `write()` and the process died before `fsync` — the case that decides
    /// whether this feature can lose a ride.
    BeforeObjectFsync,
    /// The ride's two files are durable and the process died before the index committed.
    BeforeIndexCommit,
    /// [`Library::migrate`] moved every `.obcride` and died before the index followed — the
    /// half-migrated state a restart must be able to finish from.
    MigrateBeforeIndexMove,
}

/// The message a simulated crash returns, so a test can tell it from a real IO error.
const CRASH_MSG: &str = "simulated power loss";

// ============================ the library ============================

pub struct Library {
    /// The visible, relocatable GPX folder.
    root: PathBuf,
    /// The internal archive: `index.json` plus the `.obcride` objects. **Not** relocatable — it is
    /// app data, and it does not move when the rider moves the GPX folder.
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
    /// A missing index is an empty library. A **corrupt** index is also an empty library, and that
    /// is the safe direction rather than a shrug: the worst it costs is re-downloading rides the
    /// device still holds (the ack is add-only and `synced_at` is first-ack-wins, so nothing is
    /// disturbed there), while any attempt to salvage a half-parsed index risks writing over the
    /// files it half-understood.
    fn load(&self) -> Index {
        read_index(&self.archive.join(INDEX_FILE))
            .unwrap_or_else(|| Index { version: INDEX_VERSION, rides: Vec::new() })
    }

    /// A stored record, joined to the filesystem as it is right now.
    fn entry(&self, ride: LibraryRide) -> RideEntry {
        let ride_path = self.archive.join(&ride.ride_file);
        let gpx_path = self.root.join(&ride.gpx_file);
        // Existence alone is not durability: `present` feeds `durable_ids`, which feeds the ack,
        // so a truncated or swapped archive file must read as *absent* — the ride is then pulled
        // again, which is the direction that costs a download instead of a ride. The index's own
        // `bytes` is the cheap whole-file check (the CRC is there too, but hashing every archive
        // on every read would make listing a library O(bytes)).
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
        IndexView {
            folder: self.root.display().to_string(),
            is_default,
            rides: self.entries(),
            migration_warning: None,
        }
    }

    /// The ride ids of `(serial, epoch)` whose object is **in the archive right now** — the exact
    /// set this app is entitled to ack.
    ///
    /// This is the one function that answers "what is durably here", and the frontend acks its
    /// result rather than the set of rides it thinks it just wrote. That is deliberate: a ride whose
    /// import failed, whose archive file is gone, or whose write was interrupted by a power cut is
    /// absent from this list and therefore never flagged on the device. Re-sending the whole list
    /// every pull is also what heals a device that lost its `/tracks/SYNCED.SET` (§4.4: an ack is
    /// add-only, and unknown ids are ignored).
    pub fn durable_ids(&self, serial: &str, epoch: u32) -> Vec<u16> {
        let _guard = lock();
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
        let _guard = lock();
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
        std::fs::create_dir_all(&self.archive).map_err(|e| format!("create {}: {e}", self.archive.display()))?;
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

        // Order is the contract: the lossless object (archive), then the portable GPX (the visible
        // folder), then the index that claims both. A crash at any point leaves an index that does
        // not name this ride.
        durable_write(&self.archive, &ride_file, &req.object, self.crash_at(CrashPoint::BeforeObjectFsync))
            .map_err(|e| format!("write {}: {e}", self.archive.join(&ride_file).display()))?;
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
        let _guard = lock();
        let ride = self.find(key)?;
        std::fs::read(self.archive.join(&ride.ride_file))
            .map_err(|e| format!("read {}: {e}", self.archive.join(&ride.ride_file).display()))
    }

    /// (Re-)write one ride's GPX durably into the visible folder — the automatic repair for a GPX
    /// somebody deleted or renamed. The archived object is the source, so this can always be run
    /// again.
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

    /// Rewrite the index durably. Its own temp-and-rename, so a crash mid-write leaves the previous
    /// index whole rather than a truncated one — losing the *last* ride's record, never the rest.
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

    // ============================ migration ============================

    /// Move a pre-split library (index and `.obcride` archives in the visible folder) into the
    /// archive directory. Idempotent, cheap when there is nothing to do, and safe to interrupt.
    ///
    /// The rules, in the order they earn their keep:
    ///
    /// * **Only provably-ours files move**: `index.json` and `*.obcride`, by exact name. `.gpx`
    ///   files — and anything else a person put in their own folder — are never touched.
    /// * **Every move is durable**: `rename` where the filesystem allows it (same volume, atomic),
    ///   otherwise copy → fsync → rename-into-place ([`durable_write`]) and only then unlink the
    ///   source. App data and a user-chosen folder can be different filesystems, so the copy path
    ///   is a first-class citizen, not an error branch.
    /// * **The index moves last.** [`Library::load`] reads only the archive's index, so a crash
    ///   mid-migration leaves an app that reports an empty library and acks nothing — the safe
    ///   direction — and the next open finds the visible index still in place and finishes the job.
    /// * **A half-migrated pair of indexes merges as a union.** If both the archive and the visible
    ///   folder hold an index (a crash between the archive index landing and the source unlinking,
    ///   or an import that ran between two migration attempts), every key from both survives; on a
    ///   key collision the archive's record wins, because post-split writes go there. Nothing is
    ///   ever dropped.
    /// * **A same-named archive file is "already moved" only if the bytes match.** Two different
    ///   rides can share one basename across two indexes (a restored backup, a second machine's
    ///   relocated folder). Treating bare name-existence as "done" would delete the only copy of
    ///   one of them — and then ack it. A mismatch re-homes the source under a fresh name and
    ///   re-points its record.
    pub fn migrate(&self) -> Result<(), String> {
        let _guard = lock();
        if self.archive == self.root {
            return Ok(()); // degenerate configuration; nothing to split.
        }
        // An unreadable/absent visible folder has nothing to migrate — including the fresh-install
        // case and a relocated folder on an unplugged drive.
        let Some((has_index, mut ride_files)) = legacy_files(&self.root) else {
            return Ok(());
        };
        if !has_index && ride_files.is_empty() {
            return Ok(());
        }

        std::fs::create_dir_all(&self.archive).map_err(|e| format!("create {}: {e}", self.archive.display()))?;
        ride_files.sort();
        // Basenames the migration had to change, old → new, applied to the index records below.
        let mut renames: Vec<(String, String)> = Vec::new();
        for name in &ride_files {
            let source = self.root.join(name);
            let target = self.archive.join(name);
            if target.is_file() {
                if same_contents(&source, &target)? {
                    // A leftover from an interrupted earlier run: the archive copy is byte-for-byte
                    // this file, so removing the source is the *completion* of that move.
                    std::fs::remove_file(&source).map_err(|e| format!("remove {}: {e}", source.display()))?;
                } else {
                    // Same basename, different bytes: a restored backup, or a second machine's
                    // folder — two different rides whose stems collided across two indexes. The
                    // one thing this must never be read as is "already moved": deleting the source
                    // here would destroy the only copy of one ride while its record unions in
                    // pointing at the other's bytes — and `durable_ids` would then ack a ride that
                    // was never stored. Re-home it under a fresh name instead, and re-point its
                    // record when the indexes merge.
                    let fresh = crate::paths::unique_in(&self.archive, name);
                    let fresh_name = fresh
                        .file_name()
                        .and_then(|n| n.to_str())
                        .ok_or_else(|| format!("no usable name beside {}", target.display()))?
                        .to_string();
                    move_file_durably(&source, &self.archive, &fresh_name)?;
                    renames.push((name.clone(), fresh_name));
                }
                continue;
            }
            move_file_durably(&source, &self.archive, name)?;
        }
        if self.crash == CrashPoint::MigrateBeforeIndexMove {
            return Err(format!("{CRASH_MSG} before the index moved"));
        }

        if has_index {
            let source = self.root.join(INDEX_FILE);
            match read_index(&source) {
                Some(mut visible) => {
                    // Records whose file was re-homed above follow it by name.
                    for ride in &mut visible.rides {
                        if let Some((_, to)) = renames.iter().find(|(from, _)| *from == ride.ride_file) {
                            ride.ride_file.clone_from(to);
                        }
                    }
                    // Union with whatever the archive already holds — never fewer records; on a
                    // key collision the archive's record wins (post-split writes go there).
                    let mut merged = read_index(&self.archive.join(INDEX_FILE)).unwrap_or_default();
                    for ride in visible.rides {
                        if !merged.rides.iter().any(|r| r.key == ride.key) {
                            merged.rides.push(ride);
                        }
                    }
                    merged.version = INDEX_VERSION;
                    self.commit(&merged)?;
                    std::fs::remove_file(&source).map_err(|e| format!("remove {}: {e}", source.display()))?;
                }
                // The visible index exists but cannot be parsed. It is still provably ours and is
                // preserved, never deleted: as the archive's index if that slot is free (load()
                // treats it as empty), otherwise parked beside it under a name nothing reads.
                None => {
                    if self.archive.join(INDEX_FILE).is_file() {
                        let parked = crate::paths::unique_in(&self.archive, "pre-split-index.json");
                        let parked_name = parked
                            .file_name()
                            .and_then(|n| n.to_str())
                            .ok_or_else(|| format!("no usable name in {}", self.archive.display()))?
                            .to_string();
                        move_file_durably(&source, &self.archive, &parked_name)?;
                    } else {
                        move_file_durably(&source, &self.archive, INDEX_FILE)?;
                    }
                }
            }
        }
        // Make the unlinks in the visible folder durable too — best effort, same rule as every
        // rename: the entry that went away should stay away.
        let _ = sync_dir(&self.root);
        Ok(())
    }

    /// Whether the visible folder still holds pre-split library files — i.e. [`Library::migrate`]
    /// has work it has not managed to finish. The relocation command refuses while this is true:
    /// re-pointing the root would orphan those files permanently.
    pub fn has_unmigrated(&self) -> bool {
        let _guard = lock();
        if self.archive == self.root {
            return false;
        }
        legacy_files(&self.root).is_some_and(|(has_index, rides)| has_index || !rides.is_empty())
    }
}

/// The pre-split files in `root` that belong to this module: whether an `index.json` is there, and
/// every `*.obcride` basename. `None` when the folder cannot be read at all.
fn legacy_files(root: &Path) -> Option<(bool, Vec<String>)> {
    let entries = std::fs::read_dir(root).ok()?;
    let mut has_index = false;
    let mut ride_files = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
        if name == INDEX_FILE {
            has_index = true;
        } else if !name.starts_with('.') && Path::new(&name).extension().is_some_and(|e| e == RIDE_EXT) {
            ride_files.push(name);
        }
    }
    Some((has_index, ride_files))
}

/// Whether two files hold the same bytes. Length first (free), then the bytes themselves — an
/// `.obcride` is at most a few hundred kilobytes, so reading both is cheaper than being wrong.
fn same_contents(a: &Path, b: &Path) -> Result<bool, String> {
    let meta_a = std::fs::metadata(a).map_err(|e| format!("stat {}: {e}", a.display()))?;
    let meta_b = std::fs::metadata(b).map_err(|e| format!("stat {}: {e}", b.display()))?;
    if meta_a.len() != meta_b.len() {
        return Ok(false);
    }
    let bytes_a = std::fs::read(a).map_err(|e| format!("read {}: {e}", a.display()))?;
    let bytes_b = std::fs::read(b).map_err(|e| format!("read {}: {e}", b.display()))?;
    Ok(bytes_a == bytes_b)
}

/// Read and parse an index file; `None` for missing or unreadable (the caller decides what that
/// means — [`Library::load`] treats both as empty).
fn read_index(path: &Path) -> Option<Index> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice::<Index>(&bytes) {
        Ok(mut index) => {
            index.version = INDEX_VERSION;
            Some(index)
        }
        Err(e) => {
            eprintln!("ride library: {} is unreadable ({e}); treating it as empty", path.display());
            None
        }
    }
}

/// Move one file into `dir/name` so that no point of interruption loses it: `rename` where the
/// filesystem allows (same volume), otherwise copy + fsync + atomic-rename-into-place and only
/// then unlink the source. The copy path is what makes a cross-filesystem move (app data on one
/// volume, a relocated folder on another) exactly as safe as the same-volume one.
fn move_file_durably(source: &Path, dir: &Path, name: &str) -> Result<(), String> {
    if std::fs::rename(source, dir.join(name)).is_ok() {
        return sync_dir(dir).map_err(|e| format!("sync {}: {e}", dir.display()));
    }
    let bytes = std::fs::read(source).map_err(|e| format!("read {}: {e}", source.display()))?;
    durable_write(dir, name, &bytes, CrashPoint::None)
        .map_err(|e| format!("write {}: {e}", dir.join(name).display()))?;
    std::fs::remove_file(source).map_err(|e| format!("remove {}: {e}", source.display()))
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

/// Move the visible library — the GPX files — to a new folder.
///
/// Only the GPX files: the archive and the index are app data and stay where they are, which is
/// what makes this move cheap and boring where it used to be delicate. Refuses nesting for the same
/// reason a move into itself is not a move, and refuses to overwrite: a same-named file already at
/// the destination is someone else's file, and this function must never be the thing that replaced
/// it.
///
/// Each file is `rename`d where the filesystem allows it (same volume, instant and atomic) and
/// otherwise copied durably and then unlinked — never unlinked before the copy is fsynced, which is
/// the whole difference between relocating a library and losing one. The caller only re-points the
/// app after `Ok`, so a failure partway leaves the app reading the old folder; the GPX files that
/// did move read as missing there and are quietly re-exported from the archive, which still holds
/// every ride.
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

    // Two passes: the first so a collision is discovered before anything has moved (all-or-nothing
    // for the user), the second re-checked per file because `rename` clobbers and std has no
    // portable no-clobber rename. The re-check narrows the TOCTOU window to the one rename; a file
    // another program drops into that window can still lose — documented residual race, and the
    // library lock already rules out this process racing itself.
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
    // Best effort: an empty folder left behind is untidy, a failed move is not. A folder that
    // still holds the rider's other files simply stays.
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

    /// A library over `<base>/rides` (visible) and `<base>/archive` (internal) — the production
    /// shape, in a sandbox.
    fn library(base: &Path) -> Library {
        Library::new(base.join("rides"), base.join("archive"))
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

    /// Lay down the **pre-split** layout builds before this one wrote: index, `.obcride` and `.gpx`
    /// all in the one visible folder. Built by running the real importer with the archive pointed
    /// at the visible folder — the old code path, not a hand-forged fixture.
    fn legacy_library(folder: &Path, requests: &[ImportRequest]) -> Vec<Imported> {
        let old = Library::new(folder.to_path_buf(), folder.to_path_buf());
        requests.iter().map(|req| old.import(req).expect("legacy import")).collect()
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
        assert_eq!(lib.durable_ids("OBC-24-000317", 0xa1b2c3d4), vec![7]);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The split itself: the visible folder holds GPX and nothing else; the archive holds the
    /// object and the index.
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

    /// **Acceptance #2, the one that decides whether this feature can lose a ride.**
    ///
    /// The power goes out between `write()` and `fsync()`. The real `import` runs; what is checked
    /// is what a *restart* then sees, because that is what the ack list is computed from. The ride
    /// must be absent from `durable_ids` — which is the frontend's ack list — so the device is
    /// never told, and the next pull fetches the ride again.
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

        // Restart: a brand-new Library over the same folders, reading the same index a relaunched
        // app would read.
        let restarted = library(&base);
        assert_eq!(
            restarted.durable_ids("OBC-24-000317", 7),
            vec![1],
            "only the fsynced ride is ackable; the interrupted one is not"
        );
        assert!(
            !base.join("archive").join(format!("2025-12-01-Interrupted.{RIDE_EXT}")).exists(),
            "nothing was committed under the destination name"
        );
        assert_eq!(restarted.load().rides.len(), 1, "the index never learned about the lost ride");

        // …and the next pull, with the power on, lands it and only then makes it ackable.
        library(&base).import(&lost).expect("the retry lands");
        assert_eq!(library(&base).durable_ids("OBC-24-000317", 7), vec![1, 2]);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The other half of the ordering: the files are durable and the *index* never committed. The
    /// ride is still not ackable — the index is what `durable_ids` reads — and the retry repairs
    /// the record without minting a second one.
    #[test]
    fn a_crash_before_the_index_commits_also_leaves_the_ride_unacked() {
        let base = temp("crash-index");
        let req = request("OBC-24-000317", 7, 3, "Half landed");

        Library::crashing_at(base.join("rides"), base.join("archive"), CrashPoint::BeforeIndexCommit)
            .import(&req)
            .expect_err("crash");
        let restarted = library(&base);
        assert!(restarted.durable_ids("OBC-24-000317", 7).is_empty(), "an uncommitted index acks nothing");
        assert!(base.join("archive").join(format!("2025-12-01-Half landed.{RIDE_EXT}")).exists(), "the bytes did land");

        let retry = restarted.import(&req).expect("the retry commits");
        assert!(retry.imported);
        assert_eq!(restarted.load().rides.len(), 1, "the retry did not mint a second record");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// **Acceptance #3.** A chip erase re-mints the store epoch and the device starts assigning ids
    /// from 1 again, so a *new* ride arrives under an id an *old* ride already used. Both must
    /// survive: bare-id dedupe would silently discard the new one, which is the 2026-07-12 incident
    /// the iOS `LibraryScopingE2ETests` replays.
    #[test]
    fn an_epoch_bump_with_a_recycled_id_keeps_both_rides() {
        let base = temp("epoch");
        let lib = library(&base);
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

        let _ = std::fs::remove_dir_all(&base);
    }

    /// **Acceptance #4.** What the device is told matches what is on the disk — including after
    /// something deletes an archive file, which `present` must notice.
    #[test]
    fn durable_ids_follow_the_filesystem_not_the_index() {
        let base = temp("present");
        let lib = library(&base);
        let serial = "OBC-24-000317";
        for id in [4u16, 5, 6] {
            lib.import(&request(serial, 9, id, &format!("Ride {id}"))).expect("import");
        }
        assert_eq!(lib.durable_ids(serial, 9), vec![4, 5, 6]);

        let gone = lib.load().rides.iter().find(|r| r.object_id == 5).expect("ride 5").ride_file.clone();
        std::fs::remove_file(base.join("archive").join(&gone)).expect("delete the archive file");

        assert_eq!(lib.durable_ids(serial, 9), vec![4, 6], "a deleted ride is not durable and is not acked");
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
        assert!(lib.durable_ids("S", 1).is_empty(), "and acks nothing — the safe direction");
        // The files are still there; the next pull re-imports over them.
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

    // ---------------------------- migration ----------------------------

    /// The one-time migration over the **default** folder: a pre-split library (index, `.obcride`
    /// and `.gpx` all visible) becomes GPX-only, with the archive holding the rest — and nothing
    /// that is not provably ours is touched.
    #[test]
    fn migration_empties_the_visible_folder_of_everything_but_gpx() {
        let base = temp("migrate");
        let folder = base.join("rides");
        std::fs::create_dir_all(&folder).expect("folder");
        let landed = legacy_library(&folder, &[request("S", 1, 1, "Old one"), request("S", 1, 2, "Old two")]);
        // The rider's own files, which the migration must leave exactly alone.
        std::fs::write(folder.join("notes.txt"), b"mine").expect("stranger file");
        std::fs::write(folder.join("holiday.gpx"), b"<gpx/>").expect("stranger gpx");

        let lib = library(&base);
        lib.migrate().expect("migrate");

        let mut expected: Vec<String> =
            landed.iter().map(|l| l.ride.gpx_file.clone()).chain(["holiday.gpx".into(), "notes.txt".into()]).collect();
        expected.sort();
        assert_eq!(file_names(&folder), expected, "the visible folder is GPX (and the rider's own files) only");
        let mut archived: Vec<String> =
            landed.iter().map(|l| l.ride.ride_file.clone()).chain([INDEX_FILE.to_string()]).collect();
        archived.sort();
        assert_eq!(file_names(&base.join("archive")), archived);

        // The library reads exactly what it read before the move.
        assert_eq!(lib.durable_ids("S", 1), vec![1, 2]);
        for entry in lib.entries() {
            assert!(entry.present && entry.gpx_present, "{}: both halves survived the move", entry.ride.key);
        }
        assert_eq!(
            lib.read_object(&landed[0].ride.key).expect("archive readable"),
            request("S", 1, 1, "Old one").object
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The same move for a **relocated** folder — the pointer file keeps working, and migration
    /// runs wherever it points.
    #[test]
    fn migration_covers_a_relocated_folder_and_reruns_as_a_no_op() {
        let base = temp("migrate-reloc");
        let config = base.join("config");
        let chosen = base.join("external-drive").join("my-rides");
        std::fs::create_dir_all(&chosen).expect("folder");
        remember(&config, &chosen).expect("remember");
        legacy_library(&chosen, &[request("S", 3, 9, "Relocated ride")]);

        let folder = configured(&config).expect("the pointer survives");
        assert_eq!(folder, chosen);
        let lib = Library::new(folder, base.join("archive"));
        lib.migrate().expect("migrate");
        assert_eq!(lib.durable_ids("S", 3), vec![9]);
        assert_eq!(file_names(&chosen), vec![format!("2025-12-01-Relocated ride.{GPX_EXT}")]);

        // Idempotent: a second run finds nothing to move and changes nothing.
        let before = file_names(&base.join("archive"));
        lib.migrate().expect("re-run");
        assert_eq!(file_names(&base.join("archive")), before);
        assert_eq!(lib.durable_ids("S", 3), vec![9]);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// **The migration's crash-safety claim.** The power dies after the `.obcride`s moved and
    /// before the index followed. The restarted app reads an empty library — so it acks nothing,
    /// the safe direction — and the next `migrate()` finishes the job with nothing lost.
    #[test]
    fn a_crash_between_archive_move_and_index_move_re_migrates_cleanly() {
        let base = temp("migrate-crash");
        let folder = base.join("rides");
        std::fs::create_dir_all(&folder).expect("folder");
        let landed = legacy_library(&folder, &[request("S", 1, 1, "Caught mid-move")]);

        let err = Library::crashing_at(folder.clone(), base.join("archive"), CrashPoint::MigrateBeforeIndexMove)
            .migrate()
            .expect_err("the simulated crash");
        assert!(err.contains(CRASH_MSG), "unexpected failure: {err}");

        // The half-migrated state: object in the archive, index still in the visible folder.
        assert!(base.join("archive").join(&landed[0].ride.ride_file).exists());
        assert!(folder.join(INDEX_FILE).exists());

        // Restart. Before the re-run completes, the library must claim *nothing* — an ack from
        // this state would flag a ride the index cannot name.
        let restarted = library(&base);
        assert!(restarted.load().rides.is_empty(), "no archive index yet, so an empty library");
        assert!(restarted.durable_ids("S", 1).is_empty(), "…and nothing is ackable from it");

        restarted.migrate().expect("the re-run completes");
        assert_eq!(restarted.durable_ids("S", 1), vec![1]);
        assert!(!folder.join(INDEX_FILE).exists(), "the visible index has retired");
        assert_eq!(file_names(&folder), vec![landed[0].ride.gpx_file.clone()]);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Half-migrated *both-indexes* case: an import ran against the archive while the visible
    /// folder still had its old index (or a crash fell between the archive index landing and the
    /// source unlinking). The merge is a union — no record from either side is lost.
    #[test]
    fn two_indexes_merge_as_a_union_never_losing_a_record() {
        let base = temp("migrate-merge");
        let folder = base.join("rides");
        std::fs::create_dir_all(&folder).expect("folder");
        // The old library holds rides 1 and 2…
        legacy_library(&folder, &[request("S", 1, 1, "Old one"), request("S", 1, 2, "Old two")]);
        // …and the new-layout archive already holds ride 3 (and its own record of nothing else).
        let lib = library(&base);
        lib.import(&request("S", 1, 3, "Already split")).expect("post-split import");

        lib.migrate().expect("migrate merges");
        assert_eq!(lib.durable_ids("S", 1), vec![1, 2, 3], "the union: nothing lost from either index");
        assert!(!folder.join(INDEX_FILE).exists());
        assert_eq!(lib.load().rides.len(), 3);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// **The collision that must never read as "already moved".** The archive and a legacy folder
    /// each hold a *different* ride under the *same* basename (two indexes minted the same stem —
    /// a restored backup, a second machine's folder). The legacy file is re-homed under a fresh
    /// name and its record follows it: both rides stay readable, both ack, and neither's bytes
    /// were deleted or shadowed.
    #[test]
    fn a_same_named_archive_with_different_bytes_is_re_homed_not_deleted() {
        let base = temp("migrate-collide");
        let folder = base.join("rides");
        std::fs::create_dir_all(&folder).expect("folder");
        // The legacy library holds ride S:1:1 under the stem "2025-12-01-Twin"…
        let legacy = legacy_library(&folder, &[request("S", 1, 1, "Twin")]);
        // …and the archive already holds a different ride, whose fresh index also minted
        // "2025-12-01-Twin" (different key, different bytes, same date and name).
        let lib = library(&base);
        let archived = lib.import(&request("S2", 2, 2, "Twin")).expect("post-split import");
        assert_eq!(legacy[0].ride.ride_file, archived.ride.ride_file, "the setup really collides");

        lib.migrate().expect("migrate");

        // Both rides, both durably ackable, each reading its *own* bytes.
        assert_eq!(lib.durable_ids("S", 1), vec![1]);
        assert_eq!(lib.durable_ids("S2", 2), vec![2]);
        assert_eq!(lib.read_object(&legacy[0].ride.key).expect("legacy bytes"), request("S", 1, 1, "Twin").object);
        assert_eq!(lib.read_object(&archived.ride.key).expect("archived bytes"), request("S2", 2, 2, "Twin").object);
        // The re-homed record points at a fresh file, not the other ride's.
        let entries = lib.entries();
        let moved = entries.iter().find(|e| e.ride.key == legacy[0].ride.key).expect("record survived");
        assert_ne!(moved.ride.ride_file, archived.ride.ride_file, "the record followed the re-homed file");
        assert!(moved.present && entries.iter().all(|e| e.present));

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The benign half of the same check: identical bytes under one basename are the residue of an
    /// interrupted earlier run, and removing the source *completes* that move.
    #[test]
    fn an_identical_leftover_duplicate_is_completed_not_duplicated() {
        let base = temp("migrate-dup");
        let folder = base.join("rides");
        std::fs::create_dir_all(&folder).expect("folder");
        let legacy = legacy_library(&folder, &[request("S", 1, 1, "Copied already")]);
        // Simulate the crash-after-copy-before-unlink state by hand.
        std::fs::create_dir_all(base.join("archive")).expect("archive");
        std::fs::copy(folder.join(&legacy[0].ride.ride_file), base.join("archive").join(&legacy[0].ride.ride_file))
            .expect("pre-copy");

        let lib = library(&base);
        lib.migrate().expect("migrate");
        assert!(!folder.join(&legacy[0].ride.ride_file).exists(), "the leftover source retired");
        assert_eq!(lib.durable_ids("S", 1), vec![1]);
        assert_eq!(lib.read_object(&legacy[0].ride.key).expect("bytes"), request("S", 1, 1, "Copied already").object);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// `present` is not bare existence: a truncated archive file must read as *not durable*, drop
    /// out of the ack list, and be repaired by the next pull — a re-download, never a wrong ack.
    #[test]
    fn a_truncated_archive_is_not_durable_and_is_repaired() {
        let base = temp("truncated");
        let lib = library(&base);
        let landed = lib.import(&request("S", 1, 4, "Torn")).expect("import");
        assert_eq!(lib.durable_ids("S", 1), vec![4]);

        let path = base.join("archive").join(&landed.ride.ride_file);
        let whole = std::fs::read(&path).expect("read");
        std::fs::write(&path, &whole[..whole.len() / 2]).expect("truncate");

        assert!(lib.durable_ids("S", 1).is_empty(), "a torn file acks nothing");
        assert!(!lib.entries()[0].present, "…and the UI sees it as missing");

        let repaired = lib.import(&request("S", 1, 4, "Torn")).expect("the next pull repairs it");
        assert!(!repaired.imported, "the same ride arriving again, not a new one");
        assert_eq!(lib.durable_ids("S", 1), vec![4]);
        assert_eq!(std::fs::read(&path).expect("whole again"), whole);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn has_unmigrated_reports_the_folder_until_the_move_is_done() {
        let base = temp("unmigrated");
        let folder = base.join("rides");
        std::fs::create_dir_all(&folder).expect("folder");
        let lib = library(&base);
        assert!(!lib.has_unmigrated(), "a gpx-only (or empty) folder has nothing pending");
        legacy_library(&folder, &[request("S", 1, 1, "Pending")]);
        assert!(lib.has_unmigrated(), "legacy files pending");
        lib.migrate().expect("migrate");
        assert!(!lib.has_unmigrated(), "and done");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// An unreadable legacy index is preserved, never deleted — parked beside the archive's real
    /// index under a name nothing reads — and the migration still completes.
    #[test]
    fn an_unreadable_legacy_index_is_parked_not_deleted() {
        let base = temp("migrate-corrupt-index");
        let folder = base.join("rides");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(folder.join(INDEX_FILE), b"{ not json").expect("corrupt legacy index");
        let lib = library(&base);
        lib.import(&request("S", 1, 1, "Fine")).expect("archive index exists");

        lib.migrate().expect("migrate completes past the corrupt file");
        assert!(!folder.join(INDEX_FILE).exists(), "the legacy file left the visible folder");
        assert!(base.join("archive").join("pre-split-index.json").is_file(), "…and was parked, not deleted");
        assert_eq!(lib.durable_ids("S", 1), vec![1], "the real index is untouched");
        assert!(!lib.has_unmigrated());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_fresh_or_missing_folder_migrates_to_nothing() {
        let base = temp("migrate-empty");
        let lib = library(&base);
        lib.migrate().expect("nothing to do on a folder that does not exist");
        std::fs::create_dir_all(base.join("rides")).expect("folder");
        std::fs::write(base.join("rides").join("ride.gpx"), b"<gpx/>").expect("gpx");
        lib.migrate().expect("nothing to do on a gpx-only folder");
        assert!(!base.join("archive").exists(), "no archive directory was conjured for no reason");
        let _ = std::fs::remove_dir_all(&base);
    }

    // ---------------------------- relocation ----------------------------

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
        assert_eq!(moved.durable_ids("S", 1), vec![1], "the archive did not move, so nothing was lost");
        assert!(to.join(&landed.ride.gpx_file).is_file());
        assert!(moved.entries()[0].gpx_present);
        assert!(!from.exists(), "the old folder is gone once it is empty");

        // Moving into a folder the app has never used works, and a second relocate from an empty
        // (already-moved) source is a no-op rather than an error.
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
