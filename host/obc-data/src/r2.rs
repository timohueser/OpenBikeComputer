//! The one R2 client. rclone moves the bytes; this module builds its remote from the
//! environment and owns the rules for upload, verify and delete.
//!
//! The remote exists only as `RCLONE_CONFIG_OBCR2_*` variables in the environment of the rclone
//! child. A credential is never an argument, because every process on the machine can read argv,
//! and never a connection string, because rclone splits an `https://` endpoint at its colon.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// The removal history, at the bucket root, so that no prefix delete removes it.
pub const REMOVAL_LOG: &str = "removed.jsonl";

const REMOTE: &str = "obcr2";

/// rclone's exit status for a directory that does not exist.
const DIRECTORY_NOT_FOUND: i32 = 3;

/// The set of environment variables that names a bucket and its credential.
#[derive(Debug, Clone, Copy)]
pub enum Credentials {
    /// `OBC_R2_*`: maps, planner, terrain reference and firmware.
    Main,
    /// `OBC_FIXTURE_R2_*`: the development fixture packages.
    Fixtures,
}

impl Credentials {
    fn prefix(self) -> &'static str {
        match self {
            Self::Main => "OBC_R2",
            Self::Fixtures => "OBC_FIXTURE_R2",
        }
    }
}

/// One object in the bucket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Object {
    pub key: String,
    pub bytes: u64,
    /// The upload time that the bucket reports.
    pub modified: String,
}

/// The headers and the overwrite rule of one upload.
#[derive(Debug, Default, Clone, Copy)]
pub struct Upload<'a> {
    pub cache_control: Option<&'a str>,
    pub content_type: Option<&'a str>,
    /// An object that is already at the key must hold the same bytes; it is never replaced.
    pub immutable: bool,
}

/// A bucket on R2, or a local directory that stands in for one.
pub struct Bucket {
    root: String,
    env: Vec<(String, String)>,
    secret: Option<String>,
    describe: String,
}

impl Bucket {
    /// The bucket that the environment names, or the name of the variable that is missing.
    ///
    /// `<PREFIX>_LOCAL_DIR` replaces the bucket with a local directory, for tests.
    pub fn from_env(credentials: Credentials) -> Result<Self, String> {
        Self::from_vars(credentials, |name| std::env::var(name).ok().filter(|value| !value.is_empty()))
    }

    fn from_vars(credentials: Credentials, var: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let prefix = credentials.prefix();
        if let Some(dir) = var(&format!("{prefix}_LOCAL_DIR")) {
            return Ok(Self::local(Path::new(&dir)));
        }
        let need = |suffix: &str| {
            let name = format!("{prefix}_{suffix}");
            var(&name).ok_or_else(|| format!("{name} is not set; tools/obc.local or the environment holds it"))
        };
        let bucket = need("BUCKET")?;
        let endpoint = match var(&format!("{prefix}_ENDPOINT")) {
            Some(endpoint) => endpoint,
            None => format!("https://{}.r2.cloudflarestorage.com", need("ACCOUNT_ID")?),
        };
        let secret = need("SECRET_ACCESS_KEY")?;
        let env = [
            ("TYPE", "s3".to_string()),
            ("PROVIDER", "Cloudflare".to_string()),
            ("REGION", "auto".to_string()),
            ("ENDPOINT", endpoint.clone()),
            ("ACCESS_KEY_ID", need("ACCESS_KEY_ID")?),
            ("SECRET_ACCESS_KEY", secret.clone()),
            ("NO_CHECK_BUCKET", "true".to_string()),
        ];
        Ok(Self {
            root: format!("{REMOTE}:{bucket}"),
            env: env.into_iter().map(|(key, value)| (format!("RCLONE_CONFIG_OBCR2_{key}"), value)).collect(),
            secret: Some(secret),
            describe: format!("r2 bucket {bucket} via {endpoint}"),
        })
    }

    /// A local directory as the bucket, through rclone's `local` backend.
    pub fn local(dir: &Path) -> Self {
        let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
        Self {
            root: format!("{REMOTE}:{}", dir.display()),
            env: vec![("RCLONE_CONFIG_OBCR2_TYPE".into(), "local".into())],
            secret: None,
            describe: format!("local directory {}", dir.display()),
        }
    }

    /// Where the bucket is. It never holds a credential.
    pub fn describe(&self) -> &str {
        &self.describe
    }

    /// Every object under `prefix`.
    pub fn list(&self, prefix: &str) -> Result<Vec<Object>, String> {
        check_key(prefix)?;
        let rows = self.lsjson(format!("{}/{prefix}", self.root), &[])?;
        rows.into_iter().map(|row| row.object(&format!("{prefix}/"))).collect()
    }

    /// The objects of `keys` that the bucket holds. A key that it does not hold is absent from
    /// the map.
    pub fn stat(&self, keys: &[String]) -> Result<BTreeMap<String, Object>, String> {
        Ok(self.stat_rows(keys, false)?.into_values().map(|(object, _)| (object.key.clone(), object)).collect())
    }

    /// Download one object into `file`.
    pub fn get(&self, key: &str, file: &Path) -> Result<(), String> {
        check_key(key)?;
        self.checked(&["copyto".into(), self.path(key), file.display().to_string()]).map(drop)
    }

    /// Upload `file` to `key`. rclone skips an object that already holds the same checksum.
    pub fn put(&self, file: &Path, key: &str, upload: &Upload) -> Result<(), String> {
        check_key(key)?;
        if upload.immutable && !self.stat(&[key.to_string()])?.is_empty() {
            return self.verify(file, key).map_err(|e| format!("{e}; the object is immutable, so it is not replaced"));
        }
        self.checked(&put_args(file, &self.path(key), upload)).map(drop)
    }

    /// Prove that `key` holds the bytes of `file`: the same size, and the same MD5 when both
    /// sides know it.
    pub fn verify(&self, file: &Path, key: &str) -> Result<(), String> {
        let local = std::path::absolute(file).map_err(|e| format!("{}: {e}", file.display()))?;
        let rows = self.lsjson(local.display().to_string(), &["--hash", "--hash-type", "MD5"])?;
        let (want, want_md5) = single(rows).ok_or_else(|| format!("{}: not a file", file.display()))?;
        let (have, have_md5) = self
            .stat_rows(&[key.to_string()], true)?
            .remove(key)
            .ok_or_else(|| format!("{key}: not in {}", self.describe))?;
        if have.bytes != want.bytes {
            return Err(format!("{key}: holds {} bytes, but {} has {}", have.bytes, file.display(), want.bytes));
        }
        if let (Some(have_md5), Some(want_md5)) = (have_md5, want_md5) {
            if have_md5 != want_md5 {
                return Err(format!("{key}: MD5 {have_md5} differs from {} ({want_md5})", file.display()));
            }
        }
        Ok(())
    }

    /// Delete `objects`. The removal log goes first: a line for a delete that then fails is a
    /// smaller loss than a delete with no line.
    pub fn delete(&self, objects: &[Object], reason: &str) -> Result<(), String> {
        if objects.is_empty() {
            return Err("no object to delete".into());
        }
        for object in objects {
            check_key(&object.key)?;
        }
        let scratch = Scratch::new()?;
        let log = scratch.0.join(REMOVAL_LOG);
        if self.stat(&[REMOVAL_LOG.to_string()])?.is_empty() {
            std::fs::write(&log, "").map_err(|e| format!("{}: {e}", log.display()))?;
        } else {
            self.get(REMOVAL_LOG, &log)?;
        }
        let mut history = std::fs::read_to_string(&log).map_err(|e| format!("{}: {e}", log.display()))?;
        history.push_str(&removal_lines(objects, reason, &user(), &crate::date::now()));
        std::fs::write(&log, history).map_err(|e| format!("{}: {e}", log.display()))?;
        self.put(&log, REMOVAL_LOG, &Upload::default())?;

        let list = scratch.0.join("delete.txt");
        let keys: String = objects.iter().map(|object| format!("{}\n", object.key)).collect();
        std::fs::write(&list, keys).map_err(|e| format!("{}: {e}", list.display()))?;
        self.checked(&["delete".into(), self.root.clone(), "--files-from-raw".into(), list.display().to_string()])
            .map(drop)
    }

    fn path(&self, key: &str) -> String {
        format!("{}/{key}", self.root)
    }

    fn stat_rows(&self, keys: &[String], hash: bool) -> Result<BTreeMap<String, (Object, Option<String>)>, String> {
        for key in keys {
            check_key(key)?;
        }
        let scratch = Scratch::new()?;
        let list = scratch.0.join("keys.txt");
        std::fs::write(&list, keys.iter().map(|key| format!("{key}\n")).collect::<String>())
            .map_err(|e| format!("{}: {e}", list.display()))?;
        let mut extra = vec!["--files-from-raw", list.to_str().ok_or("scratch path is not UTF-8")?];
        if hash {
            extra.extend(["--hash", "--hash-type", "MD5"]);
        }
        let mut found = BTreeMap::new();
        for row in self.lsjson(self.root.clone(), &extra)? {
            let md5 = row.hashes.get("md5").filter(|value| !value.is_empty()).cloned();
            let object = row.object("")?;
            found.insert(object.key.clone(), (object, md5));
        }
        Ok(found)
    }

    /// The files under `path`, with their upload times. A folder that does not exist is empty.
    fn lsjson(&self, path: String, extra: &[&str]) -> Result<Vec<Row>, String> {
        let mut args: Vec<String> =
            ["lsjson", "--recursive", "--files-only", "--use-server-modtime", "--no-mimetype"].map(String::from).into();
        args.extend(extra.iter().map(|arg| arg.to_string()));
        args.push(path);
        let out = self.run(&args)?;
        if out.status.code() == Some(DIRECTORY_NOT_FOUND) {
            return Ok(Vec::new());
        }
        serde_json::from_slice(&self.success(out, "lsjson")?.stdout).map_err(|e| format!("rclone lsjson: {e}"))
    }

    fn run(&self, args: &[String]) -> Result<Output, String> {
        Command::new("rclone")
            .envs(self.env.iter().map(|(key, value)| (key, value)))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("rclone: {e}; the R2 client needs rclone on PATH (https://rclone.org/install/)"))
    }

    fn checked(&self, args: &[String]) -> Result<Output, String> {
        self.success(self.run(args)?, &args[0])
    }

    fn success(&self, out: Output, command: &str) -> Result<Output, String> {
        if out.status.success() {
            return Ok(out);
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        let reason = stderr.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("no message");
        Err(self.redact(&format!("rclone {command} failed: {reason}")))
    }

    /// A backstop: the secret is not in argv, but no rclone message may carry it into a log.
    fn redact(&self, text: &str) -> String {
        match &self.secret {
            Some(secret) if !secret.is_empty() => text.replace(secret.as_str(), "***"),
            _ => text.to_string(),
        }
    }
}

/// A key or a prefix inside the bucket: never the root, and no empty, `.` or `..` part.
fn check_key(key: &str) -> Result<(), String> {
    if key.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
        return Err(format!("{key:?} does not name an object or folder inside the bucket"));
    }
    Ok(())
}

fn put_args(file: &Path, target: &str, upload: &Upload) -> Vec<String> {
    let mut args = vec!["copyto".to_string(), "--checksum".to_string()];
    for (header, value) in [("Cache-Control", upload.cache_control), ("Content-Type", upload.content_type)] {
        if let Some(value) = value {
            args.extend(["--header-upload".to_string(), format!("{header}: {value}")]);
        }
    }
    args.extend([file.display().to_string(), target.to_string()]);
    args
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Row {
    path: String,
    size: i64,
    #[serde(default)]
    mod_time: String,
    #[serde(default)]
    hashes: BTreeMap<String, String>,
}

impl Row {
    fn object(self, prefix: &str) -> Result<Object, String> {
        let key = format!("{prefix}{}", self.path);
        let bytes = u64::try_from(self.size).map_err(|_| format!("{key}: rclone reports no size"))?;
        Ok(Object { key, bytes, modified: self.mod_time })
    }
}

fn single(rows: Vec<Row>) -> Option<(Object, Option<String>)> {
    let [row]: [Row; 1] = rows.try_into().ok()?;
    let md5 = row.hashes.get("md5").filter(|value| !value.is_empty()).cloned();
    Some((row.object("").ok()?, md5))
}

/// The lines that a delete appends to [`REMOVAL_LOG`]: who removed each object, when, its size
/// and why. The layout is that of Python's `json.dumps(sort_keys=True)`.
fn removal_lines(objects: &[Object], reason: &str, by: &str, when: &str) -> String {
    let text = |value: &str| serde_json::Value::from(value).to_string();
    objects
        .iter()
        .map(|object| {
            format!(
                "{{\"by\": {}, \"bytes\": {}, \"key\": {}, \"reason\": {}, \"removed\": {}}}\n",
                text(by),
                object.bytes,
                text(&object.key),
                text(reason),
                text(when)
            )
        })
        .collect()
}

/// The login name, in the order of Python's `getpass.getuser`: the environment, then the
/// account database.
fn user() -> String {
    let from_env = ["LOGNAME", "USER", "LNAME", "USERNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()));
    from_env
        .or_else(|| {
            let out = Command::new("id").arg("-un").output().ok().filter(|out| out.status.success())?;
            Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|name| !name.is_empty())
        })
        .unwrap_or_else(|| "unknown".into())
}

/// A temporary directory that is removed when it goes out of scope.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = format!("obc-r2-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let dir = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: BTreeMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn the_credential_rides_the_environment_and_never_argv() {
        let bucket = Bucket::from_vars(
            Credentials::Fixtures,
            vars(&[
                ("OBC_FIXTURE_R2_BUCKET", "fixtures"),
                ("OBC_FIXTURE_R2_ACCOUNT_ID", "acct"),
                ("OBC_FIXTURE_R2_ACCESS_KEY_ID", "abc"),
                ("OBC_FIXTURE_R2_SECRET_ACCESS_KEY", "hunter2"),
            ]),
        )
        .unwrap();
        let env: BTreeMap<_, _> = bucket.env.iter().cloned().collect();
        assert_eq!(env["RCLONE_CONFIG_OBCR2_ENDPOINT"], "https://acct.r2.cloudflarestorage.com");
        assert_eq!(env["RCLONE_CONFIG_OBCR2_SECRET_ACCESS_KEY"], "hunter2");
        let args = put_args(Path::new("a.bin"), &bucket.path("x/a.bin"), &Upload::default());
        assert!(!args.join(" ").contains("hunter2") && !bucket.describe().contains("hunter2"));
        assert_eq!(bucket.redact("403: secret_access_key=hunter2"), "403: secret_access_key=***");

        let missing = Bucket::from_vars(Credentials::Main, vars(&[("OBC_R2_BUCKET", "maps")])).err().unwrap();
        assert!(missing.starts_with("OBC_R2_ACCOUNT_ID is not set"), "{missing}");
    }

    #[test]
    fn upload_sets_checksum_and_cache_headers() {
        let upload = Upload {
            cache_control: Some("public, max-age=31536000, immutable"),
            content_type: Some("application/json"),
            immutable: false,
        };
        assert_eq!(
            put_args(Path::new("/tmp/a.json"), "obcr2:maps/a.json", &upload),
            [
                "copyto",
                "--checksum",
                "--header-upload",
                "Cache-Control: public, max-age=31536000, immutable",
                "--header-upload",
                "Content-Type: application/json",
                "/tmp/a.json",
                "obcr2:maps/a.json"
            ]
        );
    }

    #[test]
    fn the_removal_log_keeps_the_python_layout() {
        let object = Object { key: "uploads/a.obcm".into(), bytes: 17, modified: String::new() };
        assert_eq!(
            removal_lines(&[object], "a stray \"upload\"", "rider", "2026-01-02T03:04:05Z"),
            "{\"by\": \"rider\", \"bytes\": 17, \"key\": \"uploads/a.obcm\", \"reason\": \"a stray \\\"upload\\\"\", \
             \"removed\": \"2026-01-02T03:04:05Z\"}\n"
        );
    }

    #[test]
    fn the_bucket_root_is_never_a_key() {
        for key in ["", "/", "a//b", "./a", "a/..", "/a"] {
            assert!(check_key(key).is_err(), "{key:?}");
        }
        assert!(check_key("cell-catalog/cells").is_ok());
    }
}
