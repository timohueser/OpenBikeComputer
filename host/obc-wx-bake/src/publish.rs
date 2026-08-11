//! Atomic weather publishing: the object stores the cycle puts through.
//!
//! The obc-bake pattern (`host/obc-bake/src/publish.rs`), specialized for the weather bucket.
//! Ordering is [`crate::canonical::run_cycle`]'s: every shard object the new manifest references
//! is uploaded **and re-verified at the destination** before the one mutable
//! `wx/v2/manifest.json` is replaced, so a failure anywhere earlier leaves the previous manifest —
//! and therefore the previous, complete generation — exactly as it was. Object keys are immutable
//! per generation, so re-publishing one is a checksum-skip, which is what makes every cycle
//! idempotent.
//!
//! [`R2Store`] talks to Cloudflare R2 over the S3 API directly ([`crate::s3`]), signing each
//! request from credentials this process holds and never writes anywhere — no connection-string
//! parser, nothing secret in an `argv`, and since #1279 no child process at all.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::s3::S3;

/// Frames are immutable timestamped objects: cache them hard.
pub const FRAME_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";
/// The manifest is the one mutable object; the epic caps it at 60 s.
pub const MANIFEST_CACHE_CONTROL: &str = "public, max-age=60, must-revalidate";

#[derive(Debug, Clone)]
pub struct PlannedObject {
    pub key: String,
    pub bytes: Vec<u8>,
    pub cache_control: &'static str,
    pub content_type: &'static str,
}

/// What one [`ObjectStore::delete`] did.
///
/// Deleting is the one store operation whose *failure to find anything* is a success, so the
/// outcome is a value rather than a `()`: the sweep ([`crate::sweep`]) walks every key a retired
/// generation could hold and most cycles find every one of them, but a generation that was baked
/// before a shard grid changed, or one whose cycle died mid-publish, is genuinely short a few.
/// Deliberately **not** `Default`: the derived one would be `existed: None`, which now means "this
/// backend cannot tell" — the one answer no store should ever give by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deleted {
    /// Was there an object at that key?
    ///
    /// `Some(false)` is **not** an error: the operation's contract is that the key does not exist
    /// afterwards, and a key that already did not is that.
    ///
    /// `None` is "this backend cannot tell", and it is a value rather than a guess because S3
    /// `DeleteObject` is *defined* to be idempotent: R2 answers `204` whether or not anything was
    /// there. [`DirStore`] can tell (it stats the file it is about to unlink) and does; [`R2Store`]
    /// cannot without paying a `head` per key for a figure only a report line reads. Before #1279
    /// this field claimed to know against R2 by reading rclone's stderr for "not found" — a string
    /// match that was already wrong on rclone 1.60.1, which is precisely the class of bug the typed
    /// client exists to end. Not knowing, and saying so, is the honest replacement.
    pub existed: Option<bool>,
    /// Its length, when the backend knew it *without paying for a second round-trip*.
    ///
    /// [`DirStore`] does — one `metadata` call on a local file, which is what makes the
    /// bake-to-a-directory rehearsal in `ops/weather/RUNBOOK.md` report a real number. S3 does not:
    /// `DeleteObject` answers with no length. So this is `None` against R2 by design, and the
    /// object *count* is the number that matters there.
    pub bytes: Option<u64>,
}

/// How many objects [`R2Store`] has in flight at once.
///
/// The upload phase is the only place in the cycle where concurrency is allowed, and it is
/// **network** concurrency, not compute: the box has 4 cores and was measured at 0.7 of one while
/// publishing, because every request spent its time waiting. Eight is chosen against R2's per-
/// connection round-trip rather than the core count — enough to keep the pipe full for ~75 KB
/// objects, small enough that a burst of eight is not a self-inflicted rate-limit.
pub const UPLOAD_CONCURRENCY: usize = 8;

/// Somewhere weather objects can be put, re-checked, read back and retired.
pub trait ObjectStore {
    /// Human-readable destination with any credential redacted.
    fn describe(&self) -> String;
    fn put(&mut self, object: &PlannedObject) -> Result<(), String>;
    /// Store every object, returning only once **all** of them are durably written — or `Err`
    /// having written some unknown subset of them.
    ///
    /// This is a batch and not a loop over [`Self::put`] because it is the one phase of the cycle
    /// that may run several requests at once (#1279). The safety property it must not touch is the
    /// *phase boundary*: `run_cycle` joins here completely before it heads a single key, and heads
    /// every key before the manifest is written. Failing halfway is exactly as safe as failing on
    /// one `put` was — the manifest has not moved, so every object written is unreferenced and the
    /// previous generation still stands whole.
    ///
    /// The default is the sequential loop, which is what [`DirStore`] wants: local writes are
    /// microseconds and threads would only add contention.
    fn put_all(&mut self, objects: &[PlannedObject]) -> Result<(), String> {
        for object in objects {
            self.put(object)?;
        }
        Ok(())
    }
    /// Size of the object at `key`, or `None` if it is not there — the pre-manifest fetchability
    /// proof.
    fn head(&mut self, key: &str) -> Result<Option<u64>, String>;
    /// Read an object back (the previous manifest at cycle start). `None` if absent.
    fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>, String>;
    /// Remove one object. **The only destructive operation in this crate** (WXR8 #1247), and the
    /// reason it is a single named key and not a prefix: the caller derives every key it passes
    /// from generations a manifest named, so the worst a bug here can reach is one object of one
    /// generation, never a subtree. `crate::sweep` is the only caller, and it runs only after a
    /// new manifest is durably in place.
    fn delete(&mut self, key: &str) -> Result<Deleted, String>;
}

/// Publish into a local directory: the dry-run target, the test target, and a real one for any
/// static host that serves a directory.
pub struct DirStore {
    root: PathBuf,
}

impl DirStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn dest(&self, key: &str) -> PathBuf {
        key.split('/').fold(self.root.clone(), |path, segment| path.join(segment))
    }

    /// A key store has no directories; a filesystem does. After removing the last object under
    /// `wx/v2/<generation>/f45/`, leave no `f45/` behind, or a swept generation goes on looking
    /// present to anyone who lists this tree — including the runbook's own rehearsal step, which
    /// is a directory listing and nothing else.
    ///
    /// It walks **up from the deleted file** and stops at the first non-empty directory or at the
    /// store root, so it can only ever remove directories the store itself created and only while
    /// they hold nothing at all.
    fn prune_empty_parents(&self, from: &std::path::Path) {
        let mut parent = from.parent();
        while let Some(path) = parent {
            if path == self.root || !path.starts_with(&self.root) || std::fs::remove_dir(path).is_err() {
                return;
            }
            parent = path.parent();
        }
    }
}

impl ObjectStore for DirStore {
    fn describe(&self) -> String {
        format!("local directory {}", self.root.display())
    }

    fn put(&mut self, object: &PlannedObject) -> Result<(), String> {
        let dest = self.dest(&object.key);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        // Temp-then-rename: a reader of this directory never sees a half-written object.
        let tmp = dest.with_extension("publish-tmp");
        if let Err(error) = std::fs::write(&tmp, &object.bytes) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("{}: {error}", tmp.display()));
        }
        std::fs::rename(&tmp, &dest).map_err(|error| format!("{}: {error}", dest.display()))
    }

    fn head(&mut self, key: &str) -> Result<Option<u64>, String> {
        match std::fs::metadata(self.dest(key)) {
            Ok(metadata) => Ok(Some(metadata.len())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("{key}: {error}")),
        }
    }

    fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>, String> {
        match std::fs::read(self.dest(key)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("{key}: {error}")),
        }
    }

    fn delete(&mut self, key: &str) -> Result<Deleted, String> {
        let dest = self.dest(key);
        let bytes = std::fs::metadata(&dest).ok().map(|metadata| metadata.len());
        match std::fs::remove_file(&dest) {
            Ok(()) => {
                self.prune_empty_parents(&dest);
                Ok(Deleted { existed: Some(true), bytes })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(Deleted { existed: Some(false), bytes: None })
            }
            Err(error) => Err(format!("{key}: {error}")),
        }
    }
}

/// Cloudflare R2 (bucket `obc-wx`) over the S3 API, signed in-process ([`crate::s3`]).
///
/// The destination is read from the environment, unchanged from the rclone era so the box's
/// credential file needs no edit:
///
/// ```text
/// OBC_WX_R2_ACCOUNT_ID        Cloudflare account id (builds the endpoint)
/// OBC_WX_R2_BUCKET            bucket name (default obc-wx)
/// OBC_WX_R2_ACCESS_KEY_ID     R2 API token id
/// OBC_WX_R2_SECRET_ACCESS_KEY
/// OBC_WX_R2_ENDPOINT          optional, overrides the derived endpoint — a jurisdiction bucket
///                             (https://<account>.eu.r2.cloudflarestorage.com), or a test double
/// ```
pub struct R2Store {
    s3: S3,
}

impl R2Store {
    pub fn from_env() -> Result<Self, String> {
        let var = |name: &str| std::env::var(name).map_err(|_| format!("{name} is not set"));
        let bucket = std::env::var("OBC_WX_R2_BUCKET").unwrap_or_else(|_| "obc-wx".to_string());
        let access = var("OBC_WX_R2_ACCESS_KEY_ID")?;
        let secret = var("OBC_WX_R2_SECRET_ACCESS_KEY")?;
        let endpoint = match std::env::var("OBC_WX_R2_ENDPOINT") {
            Ok(endpoint) => endpoint,
            Err(_) => format!("https://{}.r2.cloudflarestorage.com", var("OBC_WX_R2_ACCOUNT_ID")?),
        };
        // R2 has one region and its name is "auto"; it is signed into every request all the same.
        Ok(Self { s3: S3::new(endpoint, bucket, "auto", access, secret, UPLOAD_CONCURRENCY)? })
    }
}

impl ObjectStore for R2Store {
    fn describe(&self) -> String {
        format!("r2 bucket {} via {}", self.s3.bucket(), self.s3.endpoint())
    }

    fn put(&mut self, object: &PlannedObject) -> Result<(), String> {
        self.s3.put(&object.key, &object.bytes, object.cache_control, object.content_type)
    }

    /// The upload phase, and the **only** concurrent thing this crate does outside the mosaic.
    ///
    /// A fixed set of workers pulls from one cursor over the slice; the first error stops the
    /// others from starting new work and is what the caller sees. Two properties matter, and both
    /// are structural rather than hoped for:
    ///
    /// * **It joins.** `std::thread::scope` cannot return until every worker has, so when this
    ///   returns `Ok` every object is written — there is no request still in flight to race the
    ///   `head` proofs or the manifest that [`crate::canonical::run_cycle`] does next.
    /// * **It reorders nothing.** One batch is independent immutable keys with no relationship to
    ///   each other. The ordering the epic paid for is between *phases* — objects, then their
    ///   `head` proofs, then the manifest, then the sweep — and every one of those boundaries is a
    ///   full join in `run_cycle`, unchanged.
    ///
    /// Failing halfway is exactly as safe as failing on one `put` was: the manifest has not moved,
    /// so whatever was written is unreferenced and the previous generation still stands whole.
    fn put_all(&mut self, objects: &[PlannedObject]) -> Result<(), String> {
        if objects.len() < 2 {
            return objects.iter().try_for_each(|object| self.put(object));
        }
        let next = AtomicUsize::new(0);
        let failure: Mutex<Option<String>> = Mutex::new(None);
        let s3 = &self.s3;
        std::thread::scope(|scope| {
            for _ in 0..UPLOAD_CONCURRENCY.min(objects.len()) {
                scope.spawn(|| loop {
                    // Stop taking work once anything has failed: the cycle is over either way, and
                    // spending another 200 objects' worth of requests to arrive at the same `Err`
                    // is time the next tick wants.
                    if failure.lock().expect("upload failure lock").is_some() {
                        return;
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(object) = objects.get(index) else { return };
                    if let Err(error) = s3.put(&object.key, &object.bytes, object.cache_control, object.content_type) {
                        let mut slot = failure.lock().expect("upload failure lock");
                        slot.get_or_insert(error);
                        return;
                    }
                });
            }
        });
        match failure.into_inner().expect("upload failure lock") {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn head(&mut self, key: &str) -> Result<Option<u64>, String> {
        self.s3.head(key)
    }

    /// One `GetObject`. Absence is the `404` and nothing else — the difference between a bootstrap
    /// and a torn read, which #1280 needed two round-trips and a JSON field to establish over
    /// rclone and the protocol states outright (see [`crate::s3::S3::get`], and
    /// `manifest_v2::carried_generations` for what each answer licenses).
    fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>, String> {
        self.s3.get(key)
    }

    /// One `DeleteObject` — exactly one object, never a prefix. The S3 call has no way to spell
    /// "and everything under it", which is the same structural guarantee `rclone deletefile` gave
    /// and now costs one request on a live connection instead of a process spawn.
    ///
    /// `DeleteObject` is a **free** operation on R2 (Cloudflare lists it beside `DeleteBucket` and
    /// `AbortMultipartUpload`, in neither Class A nor Class B), so a sweep may issue as many as
    /// correctness wants. It is also idempotent — 204 whether or not anything was there — which is
    /// why `existed` is `None`; see [`Deleted::existed`].
    fn delete(&mut self, key: &str) -> Result<Deleted, String> {
        self.s3.delete(key)?;
        Ok(Deleted { existed: None, bytes: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_destination_line_names_the_bucket_and_never_the_credential() {
        let store = R2Store {
            s3: S3::new(
                "https://acct.r2.cloudflarestorage.com",
                "obc-wx",
                "auto",
                "abc",
                "hunter2",
                UPLOAD_CONCURRENCY,
            )
            .expect("a well-formed endpoint"),
        };
        assert_eq!(store.describe(), "r2 bucket obc-wx via https://acct.r2.cloudflarestorage.com");
        assert!(!store.describe().contains("hunter2"), "{}", store.describe());
    }

    /// The upload phase is a batch, and the batch must put **every** object exactly once whether it
    /// runs on one thread or eight. `DirStore` takes the default sequential path; the concurrent
    /// one is `R2Store`'s and shares this contract.
    #[test]
    fn a_batch_publishes_every_object_it_was_given() {
        let root = std::env::temp_dir().join(format!("obc-wx-batch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut store = DirStore::new(&root);
        let objects: Vec<PlannedObject> = (0..17)
            .map(|index| PlannedObject {
                key: format!("wx/v2/20260810T1430Z/f0/s{index}-0.obcg"),
                bytes: vec![index as u8; index + 1],
                cache_control: FRAME_CACHE_CONTROL,
                content_type: "application/octet-stream",
            })
            .collect();
        store.put_all(&objects).expect("put_all");
        for object in &objects {
            assert_eq!(store.head(&object.key).expect("head"), Some(object.bytes.len() as u64), "{}", object.key);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Deleting is idempotent, it reports the bytes it reclaimed, and it leaves no empty
    /// generation directory standing behind the objects it removed.
    #[test]
    fn a_directory_delete_is_idempotent_and_leaves_no_husk() {
        let root = std::env::temp_dir().join(format!("obc-wx-delete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut store = DirStore::new(&root);
        let key = "wx/v2/20260810T1430Z/f45/s3-2.obcg";
        store
            .put(&PlannedObject {
                key: key.to_string(),
                bytes: vec![7u8; 11],
                cache_control: FRAME_CACHE_CONTROL,
                content_type: "application/octet-stream",
            })
            .expect("put");

        assert_eq!(store.delete(key).expect("delete"), Deleted { existed: Some(true), bytes: Some(11) });
        // Idempotent: the second call is the same request and it succeeds having found nothing.
        assert_eq!(store.delete(key).expect("delete again"), Deleted { existed: Some(false), bytes: None });
        assert!(!root.join("wx/v2/20260810T1430Z").exists(), "the swept generation left a husk of empty directories");
        assert!(root.exists(), "pruning stops at the store root");
        let _ = std::fs::remove_dir_all(&root);
    }
}
