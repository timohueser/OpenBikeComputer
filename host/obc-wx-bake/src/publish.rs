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
//! [`RcloneStore`] talks to Cloudflare R2 over the S3 API through `rclone`, with the remote
//! defined entirely by environment variables on the child process — nothing secret in argv, no
//! connection-string parser (the exact lesson the map publisher already paid for).

use std::path::PathBuf;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Deleted {
    /// Was there an object at that key? `false` is **not** an error: the operation's contract is
    /// that the key does not exist afterwards, and a key that already did not is that.
    pub existed: bool,
    /// Its length, when the backend knew it *without paying for a second round-trip*.
    ///
    /// [`DirStore`] does — one `metadata` call on a local file, which is what makes the
    /// bake-to-a-directory rehearsal in `ops/weather/RUNBOOK.md` report a real number. The rclone
    /// backend does not: S3 `DeleteObject` answers with no length, and a `head` per key would
    /// double a 216-key sweep's process spawns to learn a figure nothing acts on. So this is
    /// `None` against R2 by design, and the object *count* is the number that matters there.
    pub bytes: Option<u64>,
}

/// Somewhere weather objects can be put, re-checked, read back and retired.
pub trait ObjectStore {
    /// Human-readable destination with any credential redacted.
    fn describe(&self) -> String;
    fn put(&mut self, object: &PlannedObject) -> Result<(), String>;
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
                Ok(Deleted { existed: true, bytes })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Deleted::default()),
            Err(error) => Err(format!("{key}: {error}")),
        }
    }
}

/// Cloudflare R2 (bucket `obc-wx`) through `rclone`'s S3 backend. The ephemeral remote is
/// defined by `RCLONE_CONFIG_OBCWX_*` variables in the child environment:
///
/// ```text
/// OBC_WX_R2_ACCOUNT_ID        Cloudflare account id (builds the endpoint)
/// OBC_WX_R2_BUCKET            bucket name (default obc-wx)
/// OBC_WX_R2_ACCESS_KEY_ID     R2 API token id
/// OBC_WX_R2_SECRET_ACCESS_KEY
/// OBC_WX_R2_ENDPOINT          optional, overrides the derived endpoint — a jurisdiction bucket
///                             (https://<account>.eu.r2.cloudflarestorage.com), or a test double
/// ```
pub struct RcloneStore {
    bucket: String,
    endpoint: String,
    envs: Vec<(&'static str, String)>,
    scratch: PathBuf,
    /// The binary to spawn — always plain `rclone`, resolved on `PATH`, in production. It is a
    /// field only so the absent-vs-empty regression test can point it at a stub that reproduces
    /// one specific rclone's answers; nothing reads it from the environment.
    program: PathBuf,
}

const RCLONE_REMOTE: &str = "obcwx";

impl RcloneStore {
    pub fn from_env() -> Result<Self, String> {
        let var = |name: &str| std::env::var(name).map_err(|_| format!("{name} is not set"));
        let bucket = std::env::var("OBC_WX_R2_BUCKET").unwrap_or_else(|_| "obc-wx".to_string());
        let access = var("OBC_WX_R2_ACCESS_KEY_ID")?;
        let secret = var("OBC_WX_R2_SECRET_ACCESS_KEY")?;
        let endpoint = match std::env::var("OBC_WX_R2_ENDPOINT") {
            Ok(endpoint) => endpoint,
            Err(_) => format!("https://{}.r2.cloudflarestorage.com", var("OBC_WX_R2_ACCOUNT_ID")?),
        };
        let envs = vec![
            ("RCLONE_CONFIG_OBCWX_TYPE", "s3".to_string()),
            ("RCLONE_CONFIG_OBCWX_PROVIDER", "Cloudflare".to_string()),
            ("RCLONE_CONFIG_OBCWX_REGION", "auto".to_string()),
            ("RCLONE_CONFIG_OBCWX_ENDPOINT", endpoint.clone()),
            ("RCLONE_CONFIG_OBCWX_ACCESS_KEY_ID", access),
            ("RCLONE_CONFIG_OBCWX_SECRET_ACCESS_KEY", secret),
            ("RCLONE_CONFIG_OBCWX_NO_CHECK_BUCKET", "true".to_string()),
        ];
        let scratch = std::env::temp_dir().join(format!("obc-wx-bake-{}", std::process::id()));
        Ok(Self { bucket, endpoint, envs, scratch, program: PathBuf::from("rclone") })
    }

    fn target(&self, key: &str) -> String {
        format!("{RCLONE_REMOTE}:{}/{key}", self.bucket)
    }

    fn run(&self, args: &[String]) -> Result<std::process::Output, String> {
        std::process::Command::new(&self.program)
            .envs(self.envs.iter().map(|(name, value)| (*name, value.as_str())))
            .args(args)
            .output()
            .map_err(|error| format!("rclone: {error} — publishing needs rclone on PATH (https://rclone.org/install/)"))
    }

    /// Is there an object at `key`, and how long is it? `None` is *absent*, `Some(0)` is a real
    /// zero-byte object, and telling those two apart is this function's whole job.
    ///
    /// It is the one place absence is decided, because deciding it from stderr wording does not
    /// work. The live VPS runs Debian 13's packaged **rclone v1.60.1**, and that version answers a
    /// missing key by treating the path as an empty *directory* — no error, exit 0:
    ///
    /// ```text
    /// $ rclone size --json obcwx:obc-wx/wx/v2/manifest.json   # no such object
    /// {"count":0,"bytes":0,"sizeless":0}
    /// EXIT=0
    /// ```
    ///
    /// So `bytes` alone cannot answer the question — it is `0` for both states — and the stderr
    /// match this used to rely on never ran at all. That cost a live bootstrap: a fresh `wx/v2`
    /// prefix read back as a present-but-empty manifest, which
    /// [`crate::manifest_v2::carried_generations`] correctly refuses to publish over ("exists but
    /// did not parse as JSON"), so *every* cycle failed and the prefix could never be seeded.
    ///
    /// `count` is the discriminator, and it is the only one that works across versions: a
    /// zero-byte object is `{"count":1,"bytes":0}`, an absent one is `{"count":0,...}`. `rclone
    /// size --json` has printed `count` since it grew `--json`, long before any version that could
    /// be on a box running this service, so a missing `count` is a loud error rather than a guess.
    fn stat(&self, key: &str) -> Result<Option<u64>, String> {
        let args = vec!["size".to_string(), "--json".to_string(), self.target(key)];
        let out = self.run(&args)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            // Kept as a *second* answer, not the only one: an rclone that does fail the command for
            // a missing key spells it "not found" / "directory not found", and both contain this.
            if stderr.contains("not found") {
                return Ok(None);
            }
            return Err(self.redact(&stderr));
        }
        size_json_len(key, &out.stdout)
    }

    /// Defensive backstop: a secret echoed back by a future rclone must never reach a log.
    fn redact(&self, text: &str) -> String {
        match self.envs.iter().find(|(name, _)| name.ends_with("_SECRET_ACCESS_KEY")).map(|(_, value)| value.as_str()) {
            Some(secret) if !secret.is_empty() => text.replace(secret, "***"),
            _ => text.to_string(),
        }
    }
}

impl ObjectStore for RcloneStore {
    fn describe(&self) -> String {
        format!("r2 bucket {} via {}", self.bucket, self.endpoint)
    }

    fn put(&mut self, object: &PlannedObject) -> Result<(), String> {
        // rclone copies files, so stage the bytes; `--checksum` skips an identical remote
        // object, which makes re-publishing an unchanged run's immutable frames free.
        std::fs::create_dir_all(&self.scratch).map_err(|error| format!("{}: {error}", self.scratch.display()))?;
        let staged = self.scratch.join("object.tmp");
        std::fs::write(&staged, &object.bytes).map_err(|error| format!("{}: {error}", staged.display()))?;
        let args = vec![
            "copyto".to_string(),
            "--checksum".to_string(),
            "--s3-no-check-bucket".to_string(),
            "--header-upload".to_string(),
            format!("Cache-Control: {}", object.cache_control),
            "--header-upload".to_string(),
            format!("Content-Type: {}", object.content_type),
            staged.to_string_lossy().into_owned(),
            self.target(&object.key),
        ];
        let out = self.run(&args)?;
        let _ = std::fs::remove_file(&staged);
        if !out.status.success() {
            return Err(self.redact(&String::from_utf8_lossy(&out.stderr)));
        }
        Ok(())
    }

    fn head(&mut self, key: &str) -> Result<Option<u64>, String> {
        self.stat(key)
    }

    /// `stat` first, then `cat` — because `cat` cannot answer the question on its own. On rclone
    /// v1.60.1 an absent key cats as an empty body with exit 0, indistinguishable from a real
    /// zero-byte object, and the difference is the one between a bootstrap and a torn read (see
    /// [`Self::stat`], and `manifest_v2::carried_generations` for what each one licenses).
    fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let Some(len) = self.stat(key)? else { return Ok(None) };
        let args = vec!["cat".to_string(), self.target(key)];
        let out = self.run(&args)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            // It was there a round-trip ago; if it is not now, absent is still the honest answer.
            if stderr.contains("not found") {
                return Ok(None);
            }
            return Err(self.redact(&stderr));
        }
        // A body shorter than the object is the torn read this bucket has a recorded history of,
        // and it is worth naming as one: unchecked it reaches the caller as a valid-looking short
        // document, and for the manifest that means a JSON parse error blamed on the wrong thing.
        if out.stdout.len() as u64 != len {
            return Err(format!(
                "{key}: read back {} bytes but the store reports {len} — a torn or truncated body, not a document",
                out.stdout.len()
            ));
        }
        Ok(Some(out.stdout))
    }

    /// `rclone deletefile` — exactly one object, never a prefix. Same env-only credentials and the
    /// same `redact()` path as every other call here.
    ///
    /// `DeleteObject` is a **free** operation on R2 (Cloudflare lists it beside `DeleteBucket` and
    /// `AbortMultipartUpload`, in neither Class A nor Class B), so a sweep may issue as many as
    /// correctness wants. What it does cost is a process spawn per key; see `crate::sweep` for the
    /// wall-clock that buys, and why it is paid after the manifest swap where nothing waits on it.
    fn delete(&mut self, key: &str) -> Result<Deleted, String> {
        let args = vec!["deletefile".to_string(), self.target(key)];
        let out = self.run(&args)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            // Unlike `head`/`get`, this one *can* read absence off stderr: `deletefile` genuinely
            // fails for a key that is not there, on every rclone including the v1.60.1 whose silent
            // exit-0 answers forced `stat` to exist — and absent is the outcome this call was
            // asking for anyway, so a false positive here costs nothing but a `existed: false`.
            if stderr.contains("not found") || stderr.contains("directory not found") {
                return Ok(Deleted::default());
            }
            return Err(self.redact(&stderr));
        }
        // S3 answers a delete with no length, and paying a `head` per key to learn one would double
        // the sweep's round-trips for a number nothing acts on.
        Ok(Deleted { existed: true, bytes: None })
    }
}

/// The `count`/`bytes` reading of one `rclone size --json` document. Split out from
/// [`RcloneStore::stat`] so the absent-vs-empty distinction is testable against the exact bytes a
/// real rclone printed, with no process to spawn.
fn size_json_len(key: &str, stdout: &[u8]) -> Result<Option<u64>, String> {
    let json: serde_json::Value =
        serde_json::from_slice(stdout).map_err(|error| format!("{key}: rclone size --json: {error}"))?;
    let Some(count) = json.get("count").and_then(serde_json::Value::as_i64) else {
        return Err(format!(
            "{key}: `rclone size --json` printed no `count` field ({}) — that field is how an absent object is told \
             from a zero-byte one, and every rclone with `--json` prints it",
            String::from_utf8_lossy(stdout).trim()
        ));
    };
    if count <= 0 {
        return Ok(None);
    }
    // Present, so a length it cannot state is a real failure and not an absence — the conflation
    // that used to live in `head`, where an unreadable `bytes` returned the same `None` as "gone".
    json.get("bytes")
        .and_then(serde_json::Value::as_i64)
        .and_then(|bytes| u64::try_from(bytes).ok())
        .map(Some)
        .ok_or_else(|| format!("{key}: `rclone size --json` counted {count} object(s) but printed no usable `bytes`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_credential_reaches_argv_or_a_log() {
        let store = RcloneStore {
            bucket: "obc-wx".into(),
            endpoint: "https://acct.r2.cloudflarestorage.com".into(),
            envs: vec![
                ("RCLONE_CONFIG_OBCWX_ENDPOINT", "https://acct.r2.cloudflarestorage.com".into()),
                ("RCLONE_CONFIG_OBCWX_ACCESS_KEY_ID", "abc".into()),
                ("RCLONE_CONFIG_OBCWX_SECRET_ACCESS_KEY", "hunter2".into()),
            ],
            scratch: std::env::temp_dir(),
            program: PathBuf::from("rclone"),
        };
        assert_eq!(store.target("wx/v2/manifest.json"), "obcwx:obc-wx/wx/v2/manifest.json");
        assert!(!store.describe().contains("hunter2"), "{}", store.describe());
        assert_eq!(store.redact("secret_access_key=hunter2: 403"), "secret_access_key=***: 403");
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

        assert_eq!(store.delete(key).expect("delete"), Deleted { existed: true, bytes: Some(11) });
        // Idempotent: the second call is the same request and it succeeds having found nothing.
        assert_eq!(store.delete(key).expect("delete again"), Deleted { existed: false, bytes: None });
        assert!(!root.join("wx/v2/20260810T1430Z").exists(), "the swept generation left a husk of empty directories");
        assert!(root.exists(), "pruning stops at the store root");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The exact documents rclone v1.60.1 printed on the live VPS, and what each one means.
    ///
    /// `bytes` is `0` in two of these three and they are not the same state: reading absence off
    /// `bytes` — or off the stderr that this version never writes — is what wedged the `wx/v2`
    /// bootstrap on 2026-08-11.
    #[test]
    fn count_not_bytes_tells_an_absent_object_from_an_empty_one() {
        let key = "wx/v2/manifest.json";
        // Observed: `rclone size --json` on a key that does not exist. Exit 0, no stderr.
        assert_eq!(size_json_len(key, br#"{"count":0,"bytes":0,"sizeless":0}"#), Ok(None));
        // A real, present, zero-byte object — same `bytes`, different answer.
        assert_eq!(size_json_len(key, br#"{"count":1,"bytes":0,"sizeless":0}"#), Ok(Some(0)));
        assert_eq!(size_json_len(key, br#"{"count":1,"bytes":1743,"sizeless":0}"#), Ok(Some(1743)));

        // An rclone that prints no `count` cannot answer the question at all, so it says so rather
        // than guessing an absence — the guess is the bug.
        let error = size_json_len(key, br#"{"bytes":0}"#).expect_err("no count is not an absence");
        assert!(error.contains("`count`"), "{error}");
        let error = size_json_len(key, b"").expect_err("empty stdout is not an absence");
        assert!(error.contains("rclone size --json"), "{error}");
    }

    /// End to end through the real `stat`/`get` code path, against a stub that answers exactly the
    /// way rclone v1.60.1 does — silently, exit 0, for a key that is not there.
    ///
    /// The last assertion is the live failure itself: on a fresh `wx/v2` prefix the cycle must
    /// bootstrap, not refuse to publish because it read an empty body as a torn manifest.
    #[cfg(unix)]
    #[test]
    fn a_missing_object_reads_as_absent_not_as_an_empty_body() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("obc-wx-rclone-1601-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("obc-wx/wx/v2")).expect("fixture tree");
        std::fs::write(root.join("obc-wx/wx/v2/manifest.json"), br#"{"version":2}"#).expect("manifest");
        std::fs::write(root.join("obc-wx/wx/v2/empty.json"), b"").expect("zero-byte object");

        // Debian 13's rclone, in the two answers that matter: a missing key is an empty directory
        // to `size` and an empty body to `cat`, and neither writes a word to stderr or fails.
        let stub = root.join("rclone-1.60.1-stub.sh");
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\nroot='{}'\ncmd=\"$1\"\nfor target in \"$@\"; do :; done\npath=\"$root/${{target#obcwx:}}\"\n\
                 case \"$cmd\" in\n\
                 size) if [ -f \"$path\" ]; then printf '{{\"count\":1,\"bytes\":%s,\"sizeless\":0}}\\n' \
                 \"$(wc -c < \"$path\" | tr -d ' ')\"; else printf '{{\"count\":0,\"bytes\":0,\"sizeless\":0}}\\n'; fi ;;\n\
                 cat) if [ -f \"$path\" ]; then cat \"$path\"; fi ;;\n\
                 *) echo \"stub: unsupported $cmd\" >&2; exit 2 ;;\n\
                 esac\nexit 0\n",
                root.display()
            ),
        )
        .expect("stub");
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).expect("stub +x");

        let mut store = RcloneStore {
            bucket: "obc-wx".into(),
            endpoint: "stub".into(),
            envs: Vec::new(),
            scratch: root.clone(),
            program: stub,
        };

        assert_eq!(store.get("wx/v2/manifest.json").expect("get"), Some(br#"{"version":2}"#.to_vec()));
        assert_eq!(store.head("wx/v2/manifest.json").expect("head"), Some(13));
        // Present and empty: a body, and a `Some`.
        assert_eq!(store.get("wx/v2/empty.json").expect("get empty"), Some(Vec::new()));
        assert_eq!(store.head("wx/v2/empty.json").expect("head empty"), Some(0));
        // Absent: `None` from both, where the stderr match used to return `Some(vec![])`/`Some(0)`.
        assert_eq!(store.get("wx/v2/nothing-here.json").expect("get absent"), None);
        assert_eq!(store.head("wx/v2/nothing-here.json").expect("head absent"), None);

        let mut warnings = Vec::new();
        let previous = store.get("wx/v2/nothing-here.json").expect("get absent");
        let carried = crate::manifest_v2::carried_generations(previous.as_deref(), &mut warnings)
            .expect("a fresh wx/v2 prefix must bootstrap, not fail the cycle");
        assert!(!carried.had_predecessor(), "nothing was ever promised, so nothing may be swept");
        assert!(warnings.is_empty(), "a genuine bootstrap is not a warning: {warnings:?}");

        let _ = std::fs::remove_dir_all(&root);
    }
}
