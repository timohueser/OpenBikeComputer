//! Publishing a cell bake tree: cells and satellites first, catalog root last.
//!
//! The manifest is the only file a consumer reads before it knows what exists, so
//! the publish order is the whole contract (`OBCC_Spec.md` §11): every object the
//! manifest references must be fetchable *before* the manifest that references it
//! becomes visible. Get that backwards and a rider's browser lists a region whose
//! bytes are still uploading — a 404 in the middle of a 300 MB download, on a file
//! the catalog swore was there.
//!
//! So a publish is three phases, and the ordering is enforced structurally rather
//! than by convention: [`plan`] returns the objects with the manifest last by
//! construction, [`publish`] uploads everything but the manifest, **re-checks every
//! uploaded object's size at the destination**, and only then replaces the manifest
//! as one object. A failure anywhere before that last step leaves the previous
//! manifest — and therefore the previous, complete catalog — exactly as it was.
//!
//! ## Where the bytes go
//!
//! [`ObjectStore`] has two implementations, and the split is deliberate:
//!
//! - [`DirStore`] copies into a local directory. It is the dry-run target, the test
//!   target, and a real one — a tree published to a directory can be served by any
//!   static host, and the tests exercise the identical ordering code the R2 publish
//!   uses. **No test in this crate needs a credential.**
//! - [`RcloneStore`] shells out to `rclone`, which is the deliberate choice over an
//!   S3 SDK. The Rust S3 crates that avoid an async runtime pull either a C crypto
//!   stack (`aws-lc-rs`) or a second HTTP+XML+time dependency set, for a job that is
//!   "PUT ~120 objects, some of them gigabytes". rclone already does multipart,
//!   retries, resume, checksum-skip and bandwidth limits — the properties that
//!   matter when a full DACH publish is several hundred GB — and it keeps the
//!   project's dependency graph untouched, which is the same reasoning that keeps
//!   libGEOS the only native dependency in the packer. The cost is an external
//!   binary on the publishing box; the bake box already needs 32 GB of RAM, so one
//!   `apt install rclone` is not the constraint.
//!
//! Credentials never appear in a config file, in a log line, **or in argv**: the
//! remote is defined by `RCLONE_CONFIG_*` variables in the child process's
//! environment (see [`RcloneStore`]), so nothing secret is visible to `ps` and
//! there is no connection-string parser to mis-split an `https://` endpoint.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use obc_pack::catalog::{CatalogOptions, DEFAULT_MANIFEST_NAME};
use sha2::{Digest, Sha256};

use crate::util::human_bytes;

/// Cache lifetime for the manifest. `OBCC_Spec.md` §11: at most 60 s, because a
/// consumer cannot compensate for an over-cached manifest — a fresh bake stays
/// invisible for as long as the cache says it is.
pub const MANIFEST_CACHE_CONTROL: &str = "public, max-age=60, must-revalidate";
/// Mutable producer metadata is not referenced by a catalog root, but it keeps a
/// short TTL so direct inspection never presents an old sidecar as current.
pub const MUTABLE_CACHE_CONTROL: &str = "public, max-age=3600, must-revalidate";
/// Every root-referenced object carries its SHA-256 in the published key. Such a key
/// is immutable: a later root points at a different key, while a browser still using
/// the previous root can finish against the previous bytes without a mixed-generation
/// digest failure.
pub const PINNED_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// Object category, used to select its cache policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Mutable,
    /// Digest-addressed content referenced and pinned by the root.
    Pinned,
    /// Exactly one, and always last.
    Manifest,
}

/// One object to upload.
#[derive(Debug, Clone)]
pub struct PlannedObject {
    /// Key relative to the publish root. Pinned content uses the immutable path
    /// named by the catalog; `path` remains its stable local bake-tree source.
    pub key: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub kind: ObjectKind,
}

impl PlannedObject {
    pub fn cache_control(&self) -> &'static str {
        match self.kind {
            ObjectKind::Manifest => MANIFEST_CACHE_CONTROL,
            ObjectKind::Pinned => PINNED_CACHE_CONTROL,
            ObjectKind::Mutable => MUTABLE_CACHE_CONTROL,
        }
    }

    pub fn content_type(&self) -> &'static str {
        if self.key.ends_with(".json") {
            "application/json"
        } else if self.key.ends_with(".png") {
            "image/png"
        } else {
            "application/octet-stream"
        }
    }
}

/// Somewhere objects can be put and then looked up again.
pub trait ObjectStore {
    /// Human-readable destination, with any credential redacted.
    fn describe(&self) -> String;
    /// Upload `object`. Implementations may skip an identical remote object.
    fn put(&self, object: &PlannedObject) -> Result<(), String>;
    /// Size of the object at `key`, or `None` if it is not there. Used to prove
    /// every artifact is fetchable *before* the manifest that references it lands.
    fn head(&self, key: &str) -> Result<Option<u64>, String>;
}

/// How a publish is allowed to behave.
#[derive(Debug, Clone, Copy, Default)]
pub struct PublishOptions {
    /// Generate the manifest and plan the upload; move no bytes.
    pub dry_run: bool,
    /// Report every upload and remote verification with cumulative progress.
    pub verbose: bool,
}

/// What a publish did.
#[derive(Debug, Clone)]
pub struct PublishReport {
    pub regions: Vec<String>,
    pub cells: u32,
    pub skins: usize,
    pub warnings: Vec<String>,
    pub objects: usize,
    pub bytes: u64,
}

/// Walk the tree and list every object to publish, manifest last.
///
/// The manifest is appended by this function rather than found in the tree, so
/// "last" is a property of the plan's construction and not of a sort order someone
/// could change. Dotfiles are skipped — the bake state files live beside the
/// artifacts and are local bookkeeping, never published (`OBCC_Spec.md` §2 ignores
/// them for the same reason).
fn collect(
    tree: &Path,
    dir: &Path,
    kind: ObjectKind,
    skipped: &BTreeSet<String>,
    out: &mut Vec<PlannedObject>,
) -> Result<(), String> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .map(|e| e.map(|e| e.path()).map_err(|e| format!("{}: {e}", dir.display())))
        .collect::<Result<_, _>>()?;
    entries.sort();
    for path in entries {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect(tree, &path, kind, skipped, out)?;
            continue;
        }
        let rel = path.strip_prefix(tree).map_err(|_| format!("{}: outside the tree", path.display()))?;
        let key = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
        if skipped.contains(&key) {
            continue;
        }
        let bytes = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?.len();
        out.push(PlannedObject { key, path, bytes, kind });
    }
    Ok(())
}

/// Generate the catalog into a cell tree, then publish it.
///
/// The catalog is a root plus digest-pinned satellites, so the satellites are objects like any other
/// and land in phase 1, while the root — the only document that claims they exist
/// with a given digest — is the single object swapped in last. [`plan`] already puts
/// `catalog.json` last by construction, so the satellites' ordering needs no new
/// mechanism, only that they are on disk before the plan is built. The generator
/// writes them there.
pub fn publish(
    tree: &Path,
    store: &dyn ObjectStore,
    opts: &CatalogOptions,
    publish_opts: PublishOptions,
) -> Result<PublishReport, String> {
    crate::planet::check_publishable_tree(tree)?;
    // Publishing an existing tree is enough to pick up a new preview renderer:
    // previews contain no cell data, so requiring a multi-hour rebake would only
    // couple presentation to geometry by accident.
    let seed = obc_pack::catalog::generate(tree, opts)?;
    // `obc-bake`'s supported production schema owns the canonical Teningen
    // scene. Keep the lower-level library useful for synthetic/test catalogs;
    // OBCC deliberately makes previews optional for those producers.
    if seed.root.schema.id == "bikepacking" {
        crate::previews::generate(tree, &seed.root)?;
    }
    let generated = obc_pack::catalog::generate(tree, opts)?;
    obc_pack::catalog::write_all_atomic(tree, &generated)?;

    let objects = plan(tree, &generated)?;
    let total: u64 = objects.iter().map(|o| o.bytes).sum();
    let cells: u32 = generated.root.cell_index.iter().map(|c| c.cell_count).sum();
    let warnings = generated.warnings;
    let report = |warnings: Vec<String>| PublishReport {
        regions: generated.root.regions.iter().map(|r| r.id.clone()).collect(),
        cells,
        skins: generated.root.skins.len(),
        objects: objects.len(),
        bytes: total,
        warnings,
    };
    if publish_opts.dry_run {
        return Ok(report(warnings));
    }

    let (root_object, content) = objects.split_last().ok_or("nothing to publish")?;
    debug_assert_eq!(root_object.kind, ObjectKind::Manifest);
    let content_bytes: u64 = content.iter().map(|object| object.bytes).sum();
    let started = std::time::Instant::now();
    if publish_opts.verbose {
        eprintln!(
            "uploading {} content objects ({}) before the catalog root",
            content.len(),
            human_bytes(content_bytes)
        );
    }
    let mut completed_bytes = 0_u64;
    for (index, object) in content.iter().enumerate() {
        if publish_opts.verbose {
            eprintln!("upload [{:>3}/{}] {}  {}", index + 1, content.len(), human_bytes(object.bytes), object.key);
        }
        store.put(object).map_err(|e| format!("{}: {e}", object.key))?;
        completed_bytes = completed_bytes.saturating_add(object.bytes);
        if publish_opts.verbose {
            eprintln!(
                "       done  {} / {}  {}  ETA {}",
                human_bytes(completed_bytes),
                human_bytes(content_bytes),
                percent(completed_bytes, content_bytes),
                eta(started.elapsed(), completed_bytes, content_bytes)
            );
        }
    }
    if publish_opts.verbose {
        eprintln!("verifying {} remote objects before replacing catalog.json", content.len());
    }
    for (index, object) in content.iter().enumerate() {
        if publish_opts.verbose {
            eprintln!("verify [{:>3}/{}] {}", index + 1, content.len(), object.key);
        }
        match store.head(&object.key)? {
            Some(bytes) if bytes == object.bytes => {}
            Some(bytes) => {
                return Err(format!(
                    "{}: published as {bytes} bytes but the tree has {} — refusing to swap the root in",
                    object.key, object.bytes
                ))
            }
            None => return Err(format!("{}: not fetchable after upload — refusing to swap the root in", object.key)),
        }
    }
    if publish_opts.verbose {
        eprintln!("root             {}  {}", human_bytes(root_object.bytes), root_object.key);
    }
    store.put(root_object).map_err(|e| format!("{}: {e}", root_object.key))?;
    if publish_opts.verbose {
        eprintln!("published catalog root last; total elapsed {}", duration(started.elapsed()));
    }
    Ok(report(warnings))
}

fn percent(done: u64, total: u64) -> String {
    if total == 0 {
        return "100%".to_string();
    }
    format!("{}%", done.saturating_mul(100) / total)
}

fn eta(elapsed: std::time::Duration, done: u64, total: u64) -> String {
    if done == 0 || done >= total {
        return if done >= total { "0s".to_string() } else { "—".to_string() };
    }
    if elapsed.as_secs() < 3 {
        return "calculating…".to_string();
    }
    let elapsed_secs = elapsed.as_secs().max(1);
    let bytes_per_second = done / elapsed_secs;
    match (total - done).checked_div(bytes_per_second) {
        Some(seconds) => duration(std::time::Duration::from_secs(seconds)),
        None => "—".to_string(),
    }
}

fn duration(value: std::time::Duration) -> String {
    let seconds = value.as_secs();
    if seconds >= 3600 {
        format!("{}h{:02}m", seconds / 3600, seconds % 3600 / 60)
    } else if seconds >= 60 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

/// Every object of a generated cell catalog, root last.
///
/// Deliberately a whole-tree walk: a cell tree's
/// publishable set is `cells/`, `regions/`, `skins/`, `previews/` **and** `schema.json` — the last
/// of which is not optional, because it is the document the generator reads the
/// style-id assignment out of and the one a re-generation on another machine needs.
/// Walking the tree means a future producer document cannot be forgotten here.
/// Root-referenced cells, previews, and satellites are replaced in that walk by
/// the digest-addressed keys returned by the generator.
pub fn plan(tree: &Path, generated: &obc_pack::catalog::GeneratedCatalog) -> Result<Vec<PlannedObject>, String> {
    let mut objects = Vec::new();
    let skipped: BTreeSet<String> = generated
        .pinned_artifacts
        .iter()
        .map(|artifact| artifact.rel_path.clone())
        .chain(generated.satellites.iter().map(|satellite| satellite.rel_path.clone()))
        .collect();
    for dir in ["cells", "regions", "skins", crate::previews::PREVIEWS_DIR] {
        collect(tree, &tree.join(dir), ObjectKind::Mutable, &skipped, &mut objects)?;
    }
    for artifact in &generated.pinned_artifacts {
        let path = tree.join(artifact.rel_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        let bytes = verify_generated_pin(&path, artifact.bytes, &artifact.sha256)?;
        objects.push(PlannedObject { key: artifact.published_rel_path.clone(), path, bytes, kind: ObjectKind::Pinned });
    }
    for satellite in &generated.satellites {
        let path = tree.join(satellite.rel_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        let bytes = verify_generated_pin(&path, satellite.bytes, &satellite.sha256)?;
        objects.push(PlannedObject {
            key: satellite.published_rel_path.clone(),
            path,
            bytes,
            kind: ObjectKind::Pinned,
        });
    }
    let schema = tree.join("schema.json");
    if schema.is_file() {
        let bytes = std::fs::metadata(&schema).map_err(|e| format!("{}: {e}", schema.display()))?.len();
        objects.push(PlannedObject { key: "schema.json".into(), path: schema, bytes, kind: ObjectKind::Mutable });
    }
    objects.sort_by(|a, b| a.key.cmp(&b.key));

    let root = tree.join(DEFAULT_MANIFEST_NAME);
    let bytes = std::fs::metadata(&root).map_err(|e| format!("{}: {e}", root.display()))?.len();
    objects.push(PlannedObject {
        key: DEFAULT_MANIFEST_NAME.to_string(),
        path: root,
        bytes,
        kind: ObjectKind::Manifest,
    });
    Ok(objects)
}

/// Refuse to upload bytes under a digest-addressed key when the local source was
/// modified after catalog generation. Length alone is insufficient: an in-place
/// rewrite can preserve it while changing the digest.
fn verify_generated_pin(path: &Path, expected_bytes: u64, expected_sha256: &str) -> Result<u64, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let bytes = file.metadata().map_err(|e| format!("{}: {e}", path.display()))?.len();
    if bytes != expected_bytes {
        return Err(format!(
            "{}: changed from {expected_bytes} to {bytes} bytes after catalog generation",
            path.display()
        ));
    }
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(|e| format!("{}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual_sha256 = hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    if actual_sha256 != expected_sha256 {
        return Err(format!("{}: digest changed after catalog generation", path.display()));
    }
    Ok(bytes)
}

/// Publish into a local directory: the dry-run target, the test target, and a real
/// one for any static host that serves a directory.
pub struct DirStore {
    root: PathBuf,
}

impl DirStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn dest(&self, key: &str) -> PathBuf {
        key.split('/').fold(self.root.clone(), |p, seg| p.join(seg))
    }
}

impl ObjectStore for DirStore {
    fn describe(&self) -> String {
        format!("local directory {}", self.root.display())
    }

    fn put(&self, object: &PlannedObject) -> Result<(), String> {
        let dest = self.dest(&object.key);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        // Same temp-then-rename rule the manifest writer uses: a reader of this
        // directory must never see a half-copied object under its real name.
        let tmp = dest.with_extension("publish-tmp");
        if let Err(e) = std::fs::copy(&object.path, &tmp) {
            // A partial copy must not be left behind under any name — the next
            // publish would find a stray file where a served object belongs.
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("{} -> {}: {e}", object.path.display(), tmp.display()));
        }
        std::fs::rename(&tmp, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        Ok(())
    }

    fn head(&self, key: &str) -> Result<Option<u64>, String> {
        match std::fs::metadata(self.dest(key)) {
            Ok(m) => Ok(Some(m.len())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{key}: {e}")),
        }
    }
}

/// Publish to S3-compatible object storage (Cloudflare R2) through `rclone`.
///
/// The remote is defined entirely by `RCLONE_CONFIG_OBCR2_*` **environment
/// variables on the child process** — an ephemeral remote named `obcr2` that
/// exists only for that invocation. Nothing is written to disk, and nothing
/// secret rides the argument list. Both halves matter, and this replaced a
/// connection string that got neither right: argv is `ps`-visible to every
/// process on the box, and rclone's connection-string parser splits on `:`, so
/// an unquoted `endpoint=https://…` reached rclone as endpoint `https` — the
/// first real publish failed on exactly that. Environment variables have no
/// parser to appease and no process list to leak into.
///
/// ```text
/// OBC_R2_ACCOUNT_ID       Cloudflare account id (builds the endpoint)
/// OBC_R2_BUCKET           bucket name
/// OBC_R2_PREFIX           optional key prefix inside the bucket
/// OBC_R2_ACCESS_KEY_ID    R2 API token id
/// OBC_R2_SECRET_ACCESS_KEY
/// OBC_R2_ENDPOINT         optional, overrides the derived endpoint (an S3 test double)
/// ```
pub struct RcloneStore {
    bucket: String,
    prefix: String,
    /// Not a credential (it names the account, not a key) — shown in `describe`.
    endpoint: String,
    /// The child's `RCLONE_CONFIG_OBCR2_*` remote definition. The secret lives
    /// here and nowhere else.
    envs: Vec<(&'static str, String)>,
}

/// The ephemeral remote's name — matches the `RCLONE_CONFIG_OBCR2_*` variables.
const RCLONE_REMOTE: &str = "obcr2";

impl RcloneStore {
    /// Build the store from the environment, or say exactly which variable is missing.
    pub fn from_env() -> Result<Self, String> {
        let var = |name: &str| std::env::var(name).map_err(|_| format!("{name} is not set"));
        let bucket = var("OBC_R2_BUCKET")?;
        let access = var("OBC_R2_ACCESS_KEY_ID")?;
        let secret = var("OBC_R2_SECRET_ACCESS_KEY")?;
        let endpoint = match std::env::var("OBC_R2_ENDPOINT") {
            Ok(e) => e,
            Err(_) => format!("https://{}.r2.cloudflarestorage.com", var("OBC_R2_ACCOUNT_ID")?),
        };
        let prefix = std::env::var("OBC_R2_PREFIX").unwrap_or_default().trim_matches('/').to_string();
        let envs = vec![
            ("RCLONE_CONFIG_OBCR2_TYPE", "s3".to_string()),
            ("RCLONE_CONFIG_OBCR2_PROVIDER", "Cloudflare".to_string()),
            ("RCLONE_CONFIG_OBCR2_REGION", "auto".to_string()),
            ("RCLONE_CONFIG_OBCR2_ENDPOINT", endpoint.clone()),
            ("RCLONE_CONFIG_OBCR2_ACCESS_KEY_ID", access),
            ("RCLONE_CONFIG_OBCR2_SECRET_ACCESS_KEY", secret),
            ("RCLONE_CONFIG_OBCR2_NO_CHECK_BUCKET", "true".to_string()),
        ];
        Ok(Self { bucket, prefix, endpoint, envs })
    }

    fn target(&self, key: &str) -> String {
        if self.prefix.is_empty() {
            format!("{RCLONE_REMOTE}:{}/{key}", self.bucket)
        } else {
            format!("{RCLONE_REMOTE}:{}/{}/{key}", self.bucket, self.prefix)
        }
    }

    fn run(&self, args: &[String]) -> Result<std::process::Output, String> {
        std::process::Command::new("rclone")
            .envs(self.envs.iter().map(|(k, v)| (*k, v.as_str())))
            .args(args)
            .output()
            .map_err(|e| format!("rclone: {e} — the publish step needs rclone on PATH (https://rclone.org/install/)"))
    }

    /// Defensive backstop: the secret is not in argv, so rclone's output should
    /// never contain it — but if a future rclone echoes its environment into an
    /// error, it must not reach a log through us.
    fn redact(&self, text: &str) -> String {
        let secret = self
            .envs
            .iter()
            .find(|(k, _)| k.ends_with("_SECRET_ACCESS_KEY"))
            .map(|(_, v)| v.as_str())
            .filter(|s| !s.is_empty());
        match secret {
            Some(s) => text.replace(s, "***"),
            None => text.to_string(),
        }
    }
}

impl ObjectStore for RcloneStore {
    fn describe(&self) -> String {
        let where_ = if self.prefix.is_empty() { String::new() } else { format!("/{}", self.prefix) };
        format!("r2 bucket {}{where_} via {}", self.bucket, self.endpoint)
    }

    fn put(&self, object: &PlannedObject) -> Result<(), String> {
        // `copyto` with `--checksum` skips an object whose remote hash already
        // matches. Digest-addressed keys make this especially cheap: an unchanged
        // planet cell already exists at exactly its final immutable name.
        let args = vec![
            "copyto".to_string(),
            "--checksum".to_string(),
            "--s3-no-check-bucket".to_string(),
            "--header-upload".to_string(),
            format!("Cache-Control: {}", object.cache_control()),
            "--header-upload".to_string(),
            format!("Content-Type: {}", object.content_type()),
            object.path.to_string_lossy().into_owned(),
            self.target(&object.key),
        ];
        let out = self.run(&args)?;
        if !out.status.success() {
            return Err(self.redact(&String::from_utf8_lossy(&out.stderr)));
        }
        Ok(())
    }

    fn head(&self, key: &str) -> Result<Option<u64>, String> {
        let args = vec!["size".to_string(), "--json".to_string(), self.target(key)];
        let out = self.run(&args)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            // rclone reports a missing object as an error; that is a `None`, not a
            // publish failure — the caller turns it into the loud message.
            if stderr.contains("not found") || stderr.contains("directory not found") {
                return Ok(None);
            }
            return Err(self.redact(&stderr));
        }
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("{key}: {e}"))?;
        Ok(json.get("bytes").and_then(serde_json::Value::as_i64).and_then(|b| u64::try_from(b).ok()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store as `from_env` would build it, without touching the process
    /// environment (env vars are process-global and the test runner is parallel).
    fn r2_store(endpoint: &str, secret: &str) -> RcloneStore {
        RcloneStore {
            bucket: "obc-maps".into(),
            prefix: String::new(),
            endpoint: endpoint.into(),
            envs: vec![
                ("RCLONE_CONFIG_OBCR2_TYPE", "s3".into()),
                ("RCLONE_CONFIG_OBCR2_ENDPOINT", endpoint.into()),
                ("RCLONE_CONFIG_OBCR2_ACCESS_KEY_ID", "abc".into()),
                ("RCLONE_CONFIG_OBCR2_SECRET_ACCESS_KEY", secret.into()),
            ],
        }
    }

    #[test]
    fn the_endpoint_rides_the_environment_whole() {
        // The regression this store's shape exists to prevent: an `https://…`
        // endpoint in a connection string is split at the colon by rclone's
        // parser and arrives as endpoint `https`. As an environment value there
        // is no parser — assert it is carried verbatim, scheme and all.
        let store = r2_store("https://acct.r2.cloudflarestorage.com", "hunter2");
        let endpoint = store.envs.iter().find(|(k, _)| *k == "RCLONE_CONFIG_OBCR2_ENDPOINT").map(|(_, v)| v.as_str());
        assert_eq!(endpoint, Some("https://acct.r2.cloudflarestorage.com"));
    }

    #[test]
    fn no_credential_reaches_argv_or_a_log() {
        let store = r2_store("https://acct.r2.cloudflarestorage.com", "hunter2");
        // The target — the only store-derived string that becomes an argument —
        // names the ephemeral remote, never a credential.
        assert_eq!(store.target("cells/fine/1204/1052.obcm"), "obcr2:obc-maps/cells/fine/1204/1052.obcm");
        // `describe` is printed by the CLI; it carries the bucket and endpoint,
        // and neither key.
        assert!(!store.describe().contains("hunter2"), "{}", store.describe());
        assert!(!store.describe().contains("abc"), "{}", store.describe());
        // And the backstop: a secret echoed back by a future rclone dies here.
        let redacted = store.redact("Failed to copy: secret_access_key=hunter2: 403");
        assert!(!redacted.contains("hunter2"), "{redacted}");
    }

    #[test]
    fn cache_policy_follows_the_spec() {
        let manifest =
            PlannedObject { key: "catalog.json".into(), path: PathBuf::new(), bytes: 0, kind: ObjectKind::Manifest };
        let artifact = PlannedObject {
            key: "cells/fine/1204/1052.obcm".into(),
            path: PathBuf::new(),
            bytes: 0,
            kind: ObjectKind::Mutable,
        };
        let preview = PlannedObject {
            key: format!("previews/default.{}.png", "a".repeat(64)),
            path: PathBuf::new(),
            bytes: 0,
            kind: ObjectKind::Pinned,
        };
        // §7: the manifest is short-lived. Mutable producer records also revalidate;
        // root-referenced content is immutable and may be cached for a year.
        assert!(manifest.cache_control().contains("max-age=60"));
        assert!(manifest.cache_control().contains("must-revalidate"));
        assert_eq!(manifest.content_type(), "application/json");
        assert!(artifact.cache_control().contains("max-age=3600"));
        assert!(
            artifact.cache_control().contains("must-revalidate"),
            "a rewritten key must never be served from an edge without revalidating"
        );
        assert_eq!(artifact.content_type(), "application/octet-stream");
        assert_eq!(preview.content_type(), "image/png");
        assert!(preview.cache_control().contains("max-age=31536000"));
        assert!(preview.cache_control().contains("immutable"));
    }
}
