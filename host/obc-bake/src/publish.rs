//! Publishing a bake tree: artifacts first, manifest last, never in between.
//!
//! The manifest is the only file a consumer reads before it knows what exists, so
//! the publish order is the whole contract (`OBCC_Spec.md` §7): every object the
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
//! ## The shrink guard
//!
//! Ordering protects a publish from being observed half-done. It does nothing about
//! a publish that is complete and *smaller*, which is the likelier accident: the
//! manifest is generated from one tree, so publishing a partial tree — a CI run that
//! bakes only the small regions, a `--region`-narrowed local run — replaces the live
//! catalog with a strictly smaller one. No error, no half-state, and fifteen regions
//! quietly stop being offered. To a rider that is indistinguishable from a curation
//! decision, which is the exact failure this crate is built to be loud about.
//!
//! So before the first byte moves, [`publish`] reads the manifest already at the
//! destination and diffs its `(region, preset)` pairs against the new one. Anything
//! that would disappear stops the publish, names the pairs, and suggests the usual
//! cause; `allow_shrink` turns it into a loud warning for the deliberate case. See
//! [`live_manifest`] for why the live copy is read through the store rather than
//! fetched from the public URL, and why that keeps offline `dir:` publishes offline.
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
//! Credentials never appear in a config file or in a log line: the remote is built
//! as an rclone *connection string* from environment variables and passed as one
//! argument, and [`RcloneStore::describe`] redacts it.

use std::path::{Path, PathBuf};

use obc_pack::catalog::{CatalogManifest, CatalogOptions, DEFAULT_MANIFEST_NAME};

/// Cache lifetime for the manifest. `OBCC_Spec.md` §7: at most 60 s, because a
/// consumer cannot compensate for an over-cached manifest — a fresh bake stays
/// invisible for as long as the cache says it is.
pub const MANIFEST_CACHE_CONTROL: &str = "public, max-age=60, must-revalidate";
/// Artifacts get a **short** TTL with mandatory revalidation, and the reason is the
/// published layout: keys are stable paths (`regions/<id>/<preset>.obcm`, §8), not
/// content-addressed names, so every re-bake **rewrites the same key with different
/// bytes**.
///
/// Behind a CDN that is a correctness problem, not a freshness one. An edge holding
/// last week's copy of a key serves those bytes against a manifest whose `sha256` now
/// describes the new ones — and a consumer is *required* to check that digest before
/// writing to a device (§7). The mismatch is not a stale map; it is a hard download
/// failure, on every edge that cached the old object, for as long as its TTL runs.
/// An hour with `must-revalidate` bounds that: revalidation against R2 is a
/// conditional request that answers 304 in the common case, so the bytes only move
/// when they actually changed.
///
/// The deeper fix is content-addressed keys, which would make an artifact immutable
/// and cacheable forever — but the key *is* the manifest's `url`, and §8 fixes that
/// layout ("`url` is `<base-url>/<path of the artifact relative to the tree root>`"),
/// so it is a spec change rather than a knob. Left as a follow-up.
pub const ARTIFACT_CACHE_CONTROL: &str = "public, max-age=3600, must-revalidate";

/// What an object is, which is also what decides its cache policy and its order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Preset,
    Artifact,
    Sidecar,
    /// Exactly one, and always last.
    Manifest,
}

/// One object to upload.
#[derive(Debug, Clone)]
pub struct PlannedObject {
    /// Key relative to the publish root — the same path the manifest's `url` was
    /// built from, so the published layout *is* the tree layout (§8).
    pub key: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub kind: ObjectKind,
}

impl PlannedObject {
    pub fn cache_control(&self) -> &'static str {
        match self.kind {
            ObjectKind::Manifest => MANIFEST_CACHE_CONTROL,
            _ => ARTIFACT_CACHE_CONTROL,
        }
    }

    pub fn content_type(&self) -> &'static str {
        if self.key.ends_with(".json") {
            "application/json"
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
    /// Read a small object back whole — the manifest currently in place. `None`
    /// means there is none (a first publish). Only ever called for the manifest.
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String>;
}

/// How a publish is allowed to behave.
#[derive(Debug, Clone, Copy, Default)]
pub struct PublishOptions {
    /// Generate the manifest and plan the upload; move no bytes.
    pub dry_run: bool,
    /// Permit a publish that removes coverage the live catalog has (§ the shrink
    /// guard, [`coverage_lost`]). Off by default, because the normal way to lose a
    /// region is by accident.
    pub allow_shrink: bool,
}

/// What a publish did.
#[derive(Debug, Clone)]
pub struct PublishReport {
    pub manifest: CatalogManifest,
    pub warnings: Vec<String>,
    pub objects: usize,
    pub bytes: u64,
    /// `(region_id, preset_id)` pairs the live catalog served that this one does
    /// not. Non-empty only when `allow_shrink` let the publish through.
    pub coverage_lost: Vec<String>,
}

/// Walk the tree and list every object to publish, manifest last.
///
/// The manifest is appended by this function rather than found in the tree, so
/// "last" is a property of the plan's construction and not of a sort order someone
/// could change. Dotfiles are skipped — the bake state files live beside the
/// artifacts and are local bookkeeping, never published (`OBCC_Spec.md` §8 ignores
/// them for the same reason).
pub fn plan(tree: &Path) -> Result<Vec<PlannedObject>, String> {
    let mut objects = Vec::new();
    collect(tree, &tree.join("presets"), ObjectKind::Preset, &mut objects)?;
    collect(tree, &tree.join("regions"), ObjectKind::Artifact, &mut objects)?;
    objects.sort_by(|a, b| a.key.cmp(&b.key));

    let manifest = tree.join(DEFAULT_MANIFEST_NAME);
    let bytes = std::fs::metadata(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?.len();
    objects.push(PlannedObject {
        key: DEFAULT_MANIFEST_NAME.to_string(),
        path: manifest,
        bytes,
        kind: ObjectKind::Manifest,
    });
    Ok(objects)
}

fn collect(tree: &Path, dir: &Path, kind: ObjectKind, out: &mut Vec<PlannedObject>) -> Result<(), String> {
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
            collect(tree, &path, kind, out)?;
            continue;
        }
        let rel = path.strip_prefix(tree).map_err(|_| format!("{}: outside the tree", path.display()))?;
        let key = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
        let bytes = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?.len();
        let kind = if name.ends_with(".obcm.json") { ObjectKind::Sidecar } else { kind };
        out.push(PlannedObject { key, path, bytes, kind });
    }
    Ok(())
}

/// Generate the manifest into the tree, then publish the tree.
///
/// The manifest is generated here rather than taken from the tree so a publish
/// cannot ship a manifest that describes an older bake: `obc-pack catalog`'s laws
/// (every artifact's OBCM version read from its own header, every sidecar present,
/// no stray files) run on the way out, every time.
pub fn publish(
    tree: &Path,
    store: &dyn ObjectStore,
    opts: &CatalogOptions,
    publish_opts: PublishOptions,
) -> Result<PublishReport, String> {
    let generated = obc_pack::catalog::generate(tree, opts)?;
    let manifest_path = tree.join(DEFAULT_MANIFEST_NAME);
    obc_pack::catalog::write_atomic(&manifest_path, &generated.manifest)?;

    let objects = plan(tree)?;
    let total: u64 = objects.iter().map(|o| o.bytes).sum();
    let mut warnings = generated.warnings;
    if publish_opts.dry_run {
        return Ok(PublishReport {
            manifest: generated.manifest,
            warnings,
            objects: objects.len(),
            bytes: total,
            coverage_lost: Vec::new(),
        });
    }

    // Phase 0 — would this publish take coverage away? (see `coverage_lost`)
    let coverage_lost = match live_manifest(store)? {
        Some(live) => coverage_lost(&live, &generated.manifest),
        None => Vec::new(),
    };
    if !coverage_lost.is_empty() {
        if !publish_opts.allow_shrink {
            return Err(format!(
                "this publish would REMOVE {} artifact(s) the live catalog serves, and a region that stops being \
                 offered reads to a user as \"not covered\".\n\nUsually this means the tree is a partial bake — a \
                 CI run that bakes only the small regions, or a `--region`-narrowed run — being published over a \
                 full one. Publish from the full tree, or pass --allow-shrink if the removal is \
                 deliberate.\n\nWould disappear:\n{}",
                coverage_lost.len(),
                coverage_lost.iter().map(|p| format!("  {p}")).collect::<Vec<_>>().join("\n")
            ));
        }
        warnings.push(format!(
            "--allow-shrink: removing {} artifact(s) the live catalog serves: {}",
            coverage_lost.len(),
            coverage_lost.join(", ")
        ));
    }

    let (manifest_object, content) = objects.split_last().ok_or("nothing to publish")?;
    debug_assert_eq!(manifest_object.kind, ObjectKind::Manifest);

    // Phase 1 — everything the manifest will reference.
    for object in content {
        store.put(object).map_err(|e| format!("{}: {e}", object.key))?;
    }
    // Phase 2 — prove it is all there. An upload that "succeeded" but left a
    // truncated object would otherwise be discovered by a rider, mid-download.
    for object in content {
        match store.head(&object.key)? {
            Some(bytes) if bytes == object.bytes => {}
            Some(bytes) => {
                return Err(format!(
                    "{}: published as {bytes} bytes but the tree has {} — refusing to swap the manifest in",
                    object.key, object.bytes
                ))
            }
            None => {
                return Err(format!("{}: not fetchable after upload — refusing to swap the manifest in", object.key))
            }
        }
    }
    // Phase 3 — one object replacement, and the new catalog exists.
    store.put(manifest_object).map_err(|e| format!("{}: {e}", manifest_object.key))?;

    Ok(PublishReport { manifest: generated.manifest, warnings, objects: objects.len(), bytes: total, coverage_lost })
}

/// The manifest currently in place at the destination, if there is one.
///
/// Read **through the store**, from the destination itself, rather than fetched from
/// the catalog's public `base_url`. Two reasons, and both matter:
///
/// - *Authority.* The public URL is served through a CDN, so it can hand back a
///   cached manifest older than the bucket's. Diffing against that would invent
///   coverage losses that are not real (and mask ones that are).
/// - *No network where there need not be one.* A `dir:` publish — the offline
///   workstation flow and every test in this crate — answers this question by
///   reading a local file. Nothing hangs waiting for a timeout, and the guard is
///   exercised by the same code path in tests as in production.
///
/// A manifest that is present but unreadable (wrong schema version, truncated,
/// something else entirely at that key) yields `None`: it is not evidence that
/// coverage exists, so it must not block a publish that would replace it.
fn live_manifest(store: &dyn ObjectStore) -> Result<Option<CatalogManifest>, String> {
    let Some(bytes) = store.get(DEFAULT_MANIFEST_NAME)? else { return Ok(None) };
    let Ok(manifest) = serde_json::from_slice::<CatalogManifest>(&bytes) else { return Ok(None) };
    if manifest.schema_version != obc_pack::catalog::CATALOG_SCHEMA_VERSION {
        return Ok(None);
    }
    Ok(Some(manifest))
}

/// `(region, preset)` pairs the live catalog serves that the new one would not.
///
/// Pair-level rather than region-level because losing one preset of a region is the
/// same class of silent regression as losing the region: the picker simply stops
/// offering something it offered yesterday.
pub fn coverage_lost(live: &CatalogManifest, new: &CatalogManifest) -> Vec<String> {
    let incoming: std::collections::BTreeSet<(&str, &str)> =
        new.artifacts.iter().map(|a| (a.region_id.as_str(), a.preset_id.as_str())).collect();
    live.artifacts
        .iter()
        .filter(|a| !incoming.contains(&(a.region_id.as_str(), a.preset_id.as_str())))
        .map(|a| format!("{} [{}]", a.region_id, a.preset_id))
        .collect()
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

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        match std::fs::read(self.dest(key)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{key}: {e}")),
        }
    }
}

/// Publish to S3-compatible object storage (Cloudflare R2) through `rclone`.
///
/// The remote is an rclone *connection string* built from the environment, so no
/// credential is written to disk:
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
    remote: String,
    prefix: String,
    /// The remote with the secret elided, for logs.
    redacted: String,
}

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
        let common = format!(
            ":s3,provider=Cloudflare,region=auto,endpoint={endpoint},access_key_id={access},no_check_bucket=true"
        );
        Ok(Self {
            remote: format!("{common},secret_access_key={secret}:{bucket}"),
            prefix,
            redacted: format!("{common},secret_access_key=***:{bucket}"),
        })
    }

    fn target(&self, key: &str) -> String {
        if self.prefix.is_empty() {
            format!("{}/{key}", self.remote)
        } else {
            format!("{}/{}/{key}", self.remote, self.prefix)
        }
    }

    fn run(&self, args: &[String]) -> Result<std::process::Output, String> {
        std::process::Command::new("rclone")
            .args(args)
            .output()
            .map_err(|e| format!("rclone: {e} — the publish step needs rclone on PATH (https://rclone.org/install/)"))
    }
}

impl ObjectStore for RcloneStore {
    fn describe(&self) -> String {
        let where_ = if self.prefix.is_empty() { String::new() } else { format!("/{}", self.prefix) };
        format!("{}{where_}", self.redacted)
    }

    fn put(&self, object: &PlannedObject) -> Result<(), String> {
        // `copyto` with `--checksum` skips an object whose remote hash already
        // matches, which is what makes re-publishing a mostly-unchanged catalog
        // cheap; the header flags set the per-object cache policy (§7).
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
            return Err(redact(&String::from_utf8_lossy(&out.stderr), &self.remote));
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
            return Err(redact(&stderr, &self.remote));
        }
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("{key}: {e}"))?;
        Ok(json.get("bytes").and_then(serde_json::Value::as_i64).and_then(|b| u64::try_from(b).ok()))
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        // Straight from the bucket, not from the public URL: the shrink guard must
        // diff against what is actually stored, not against whatever a CDN edge
        // still has (see `live_manifest`).
        let args = vec!["cat".to_string(), self.target(key)];
        let out = self.run(&args)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stderr.contains("not found") || stderr.contains("doesn't exist") {
                return Ok(None);
            }
            return Err(redact(&stderr, &self.remote));
        }
        Ok(Some(out.stdout))
    }
}

/// Never let a connection string reach a log, however rclone chose to quote it.
fn redact(text: &str, remote: &str) -> String {
    let mut out = text.replace(remote, "<remote>");
    if let Some(idx) = out.find("secret_access_key=") {
        let tail = &out[idx + "secret_access_key=".len()..];
        let end = tail.find([',', ':']).unwrap_or(tail.len());
        out = format!("{}secret_access_key=***{}", &out[..idx], &tail[end..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_never_survives_into_an_error() {
        let remote = ":s3,access_key_id=abc,secret_access_key=hunter2:bucket";
        let text = format!("Failed to copy: {remote}/regions/x.obcm: 403");
        let redacted = redact(&text, remote);
        assert!(!redacted.contains("hunter2"), "{redacted}");
    }

    #[test]
    fn cache_policy_follows_the_spec() {
        let manifest =
            PlannedObject { key: "catalog.json".into(), path: PathBuf::new(), bytes: 0, kind: ObjectKind::Manifest };
        let artifact = PlannedObject {
            key: "regions/europe/austria/minimal.obcm".into(),
            path: PathBuf::new(),
            bytes: 0,
            kind: ObjectKind::Artifact,
        };
        // §7: the manifest is short-lived. So are the artifacts, but for a different
        // reason — their keys are stable paths that a re-bake rewrites, so an edge
        // holding old bytes against a new manifest breaks the consumer's mandatory
        // sha256 check. Both must revalidate.
        assert!(manifest.cache_control().contains("max-age=60"));
        assert!(manifest.cache_control().contains("must-revalidate"));
        assert_eq!(manifest.content_type(), "application/json");
        assert!(artifact.cache_control().contains("max-age=3600"));
        assert!(
            artifact.cache_control().contains("must-revalidate"),
            "a rewritten key must never be served from an edge without revalidating"
        );
        assert_eq!(artifact.content_type(), "application/octet-stream");
    }
}
