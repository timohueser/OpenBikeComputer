//! Atomic weather publishing: frames first, manifest last.
//!
//! The obc-bake pattern (`host/obc-bake/src/publish.rs`), specialized for the weather bucket:
//! every frame object the new manifest references is uploaded **and re-verified at the
//! destination** before the one mutable `wx/v1/manifest.json` is replaced. A failure anywhere
//! earlier leaves the previous manifest — and therefore the previous, complete weather set —
//! exactly as it was. Frame keys are immutable per upstream run, so re-publishing the same run
//! is a checksum-skip, which is what makes every cycle idempotent.
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

/// Somewhere weather objects can be put, re-checked and read back.
pub trait ObjectStore {
    /// Human-readable destination with any credential redacted.
    fn describe(&self) -> String;
    fn put(&mut self, object: &PlannedObject) -> Result<(), String>;
    /// Size of the object at `key`, or `None` if it is not there — the pre-manifest fetchability
    /// proof.
    fn head(&mut self, key: &str) -> Result<Option<u64>, String>;
    /// Read an object back (the previous manifest at cycle start). `None` if absent.
    fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>, String>;
}

/// Publish `frames` then `manifest`, in that order, with a remote existence/size check between.
/// `carried` names the already-published objects the new manifest still references (unchanged
/// products' frames): they upload nothing, but they are head-verified all the same, so a
/// lifecycle misconfiguration that expired them is caught **before** the manifest swears they
/// exist. Returns the number of objects uploaded and their byte total (manifest included).
pub fn publish(
    store: &mut dyn ObjectStore,
    frames: &[PlannedObject],
    carried: &[(String, u64)],
    manifest: &PlannedObject,
) -> Result<(usize, u64), String> {
    let mut bytes = 0u64;
    for object in frames {
        store.put(object).map_err(|error| format!("{}: {error}", object.key))?;
        bytes += object.bytes.len() as u64;
    }
    let expectations = frames
        .iter()
        .map(|object| (object.key.as_str(), object.bytes.len() as u64))
        .chain(carried.iter().map(|(key, bytes)| (key.as_str(), *bytes)));
    for (key, expected) in expectations {
        match store.head(key)? {
            Some(remote) if remote == expected => {}
            Some(remote) => {
                return Err(format!(
                    "{key}: published as {remote} bytes but the manifest expects {expected} — refusing to swap the manifest in"
                ));
            }
            None => {
                return Err(format!("{key}: not fetchable — refusing to swap the manifest in"));
            }
        }
    }
    store.put(manifest).map_err(|error| format!("{}: {error}", manifest.key))?;
    bytes += manifest.bytes.len() as u64;
    Ok((frames.len() + 1, bytes))
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
}

/// Cloudflare R2 (bucket `obc-wx`) through `rclone`'s S3 backend. The ephemeral remote is
/// defined by `RCLONE_CONFIG_OBCWX_*` variables in the child environment:
///
/// ```text
/// OBC_WX_R2_ACCOUNT_ID        Cloudflare account id (builds the endpoint)
/// OBC_WX_R2_BUCKET            bucket name (default obc-wx)
/// OBC_WX_R2_ACCESS_KEY_ID     R2 API token id
/// OBC_WX_R2_SECRET_ACCESS_KEY
/// OBC_WX_R2_ENDPOINT          optional, overrides the derived endpoint (an S3 test double)
/// ```
pub struct RcloneStore {
    bucket: String,
    endpoint: String,
    envs: Vec<(&'static str, String)>,
    scratch: PathBuf,
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
        Ok(Self { bucket, endpoint, envs, scratch })
    }

    fn target(&self, key: &str) -> String {
        format!("{RCLONE_REMOTE}:{}/{key}", self.bucket)
    }

    fn run(&self, args: &[String]) -> Result<std::process::Output, String> {
        std::process::Command::new("rclone")
            .envs(self.envs.iter().map(|(name, value)| (*name, value.as_str())))
            .args(args)
            .output()
            .map_err(|error| format!("rclone: {error} — publishing needs rclone on PATH (https://rclone.org/install/)"))
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
        let args = vec!["size".to_string(), "--json".to_string(), self.target(key)];
        let out = self.run(&args)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stderr.contains("not found") || stderr.contains("directory not found") {
                return Ok(None);
            }
            return Err(self.redact(&stderr));
        }
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|error| format!("{key}: {error}"))?;
        Ok(json.get("bytes").and_then(serde_json::Value::as_i64).and_then(|bytes| u64::try_from(bytes).ok()))
    }

    fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let args = vec!["cat".to_string(), self.target(key)];
        let out = self.run(&args)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stderr.contains("not found") || stderr.contains("directory not found") {
                return Ok(None);
            }
            return Err(self.redact(&stderr));
        }
        Ok(Some(out.stdout))
    }
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
        };
        assert_eq!(store.target("wx/v1/manifest.json"), "obcwx:obc-wx/wx/v1/manifest.json");
        assert!(!store.describe().contains("hunter2"), "{}", store.describe());
        assert_eq!(store.redact("secret_access_key=hunter2: 403"), "secret_access_key=***: 403");
    }

    #[test]
    fn a_failed_frame_verification_never_replaces_the_manifest() {
        struct BrokenStore;
        impl ObjectStore for BrokenStore {
            fn describe(&self) -> String {
                "broken".into()
            }
            fn put(&mut self, object: &PlannedObject) -> Result<(), String> {
                if object.key.ends_with("manifest.json") {
                    panic!("the manifest must never be uploaded after a failed verification");
                }
                Ok(())
            }
            fn head(&mut self, _key: &str) -> Result<Option<u64>, String> {
                Ok(None) // uploaded object is not fetchable
            }
            fn get(&mut self, _key: &str) -> Result<Option<Vec<u8>>, String> {
                Ok(None)
            }
        }
        let frame = PlannedObject {
            key: "wx/v1/x/20270101T0000Z/f0.obcg".into(),
            bytes: vec![1, 2, 3],
            cache_control: FRAME_CACHE_CONTROL,
            content_type: "application/octet-stream",
        };
        let manifest = PlannedObject {
            key: "wx/v1/manifest.json".into(),
            bytes: b"{}".to_vec(),
            cache_control: MANIFEST_CACHE_CONTROL,
            content_type: "application/json",
        };
        let error = publish(&mut BrokenStore, &[frame], &[], &manifest).unwrap_err();
        assert!(error.contains("refusing to swap the manifest in"), "{error}");
    }

    /// A carried-forward frame an unchanged product still references must be fetchable, or the
    /// manifest swap is refused — a lifecycle misconfiguration must not outrun the manifest.
    #[test]
    fn a_missing_carried_frame_never_replaces_the_manifest() {
        struct EmptyStore;
        impl ObjectStore for EmptyStore {
            fn describe(&self) -> String {
                "empty".into()
            }
            fn put(&mut self, object: &PlannedObject) -> Result<(), String> {
                if object.key.ends_with("manifest.json") {
                    panic!("the manifest must never be uploaded past a missing carried frame");
                }
                Ok(())
            }
            fn head(&mut self, _key: &str) -> Result<Option<u64>, String> {
                Ok(None)
            }
            fn get(&mut self, _key: &str) -> Result<Option<Vec<u8>>, String> {
                Ok(None)
            }
        }
        let manifest = PlannedObject {
            key: "wx/v1/manifest.json".into(),
            bytes: b"{}".to_vec(),
            cache_control: MANIFEST_CACHE_CONTROL,
            content_type: "application/json",
        };
        let carried = vec![("wx/v1/x/20270101T0000Z/f0.obcg".to_string(), 3u64)];
        let error = publish(&mut EmptyStore, &[], &carried, &manifest).unwrap_err();
        assert!(error.contains("refusing to swap the manifest in"), "{error}");
    }
}
