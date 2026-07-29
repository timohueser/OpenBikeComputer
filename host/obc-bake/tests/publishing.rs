//! The publish order, which is the only thing standing between a rider and a
//! catalog that lists a map whose bytes are still uploading.
//!
//! Every test runs against a local store: [`DirStore`] for the real copy path and a
//! recording double for the ordering assertions. **No credential, no network, no
//! rclone** — the ordering logic under test is the same code the R2 publish runs,
//! because the store is the only thing that differs between them.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use obc_bake::publish::{DirStore, ObjectKind, ObjectStore, PlannedObject, PublishOptions};
use obc_pack::catalog::CatalogOptions;

const MANIFEST: &str = "catalog.json";

fn live() -> PublishOptions {
    PublishOptions::default()
}

fn dry() -> PublishOptions {
    PublishOptions { dry_run: true, ..PublishOptions::default() }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("obc-bake-pub-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
}

/// A two-artifact bake tree, written by hand so the publisher is tested against the
/// tree *shape* the spec defines rather than against whatever a bake happened to do.
fn bake_tree(dir: &Path) -> PathBuf {
    tree_with(dir, &["europe/alpha", "europe/beta/gamma"])
}

/// The same, with an explicit region set — a "partial bake" for the shrink guard.
fn tree_with(dir: &Path, regions: &[&str]) -> PathBuf {
    let tree = dir.join("tree");
    let _ = std::fs::remove_dir_all(&tree);
    std::fs::create_dir_all(tree.join("presets")).unwrap();
    std::fs::copy(repo("builder/presets/default.json"), tree.join("presets/default.json")).unwrap();
    for region in regions {
        let region_dir = region.split('/').fold(tree.join("regions"), |p, s| p.join(s));
        std::fs::create_dir_all(&region_dir).unwrap();
        std::fs::copy(repo("apps/obc-sim/assets/monaco.obcm"), region_dir.join("default.obcm")).unwrap();
        std::fs::write(
            region_dir.join("default.obcm.json"),
            r#"{"region_name":"X","preset_version":2,"built_at":"2026-07-28T00:00:00Z","source_snapshot":"2026-07-28"}"#,
        )
        .unwrap();
        // Local bake bookkeeping — must never be published.
        std::fs::write(region_dir.join(obc_bake::bake::state_file_name("default")), "{}").unwrap();
    }
    tree
}

fn opts() -> CatalogOptions {
    CatalogOptions { base_url: "https://maps.example/obc".into(), generated_at: "2026-07-29T00:00:00Z".into() }
}

/// Records every `put` in order, and can be told to fail on one key.
struct RecordingStore {
    puts: RefCell<Vec<String>>,
    sizes: RefCell<std::collections::BTreeMap<String, u64>>,
    fail_on: Option<&'static str>,
    /// Report this key as missing at `head` time, however the `put` went.
    vanish: Option<&'static str>,
    /// The manifest already in place at this destination, if any.
    live: Option<Vec<u8>>,
}

impl RecordingStore {
    fn new() -> Self {
        Self { puts: RefCell::default(), sizes: RefCell::default(), fail_on: None, vanish: None, live: None }
    }

    fn failing_on(key: &'static str) -> Self {
        Self { fail_on: Some(key), ..Self::new() }
    }

    fn losing(key: &'static str) -> Self {
        Self { vanish: Some(key), ..Self::new() }
    }

    /// A destination that already serves `manifest`.
    fn serving(manifest: &str) -> Self {
        Self { live: Some(manifest.as_bytes().to_vec()), ..Self::new() }
    }

    fn puts(&self) -> Vec<String> {
        self.puts.borrow().clone()
    }
}

impl ObjectStore for RecordingStore {
    fn describe(&self) -> String {
        "recording store".into()
    }

    fn put(&self, object: &PlannedObject) -> Result<(), String> {
        if self.fail_on == Some(object.key.as_str()) {
            return Err("upload failed".into());
        }
        self.puts.borrow_mut().push(object.key.clone());
        self.sizes.borrow_mut().insert(object.key.clone(), object.bytes);
        Ok(())
    }

    fn head(&self, key: &str) -> Result<Option<u64>, String> {
        if self.vanish == Some(key) {
            return Ok(None);
        }
        Ok(self.sizes.borrow().get(key).copied())
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        if key == MANIFEST {
            return Ok(self.live.clone());
        }
        Ok(None)
    }
}

#[test]
fn the_manifest_is_published_last_and_the_bake_state_is_not_published_at_all() {
    let dir = scratch("order");
    let tree = bake_tree(&dir);
    let store = RecordingStore::new();

    let report = obc_bake::publish::publish(&tree, &store, &opts(), live()).expect("publish");
    assert_eq!(report.manifest.artifacts.len(), 2);

    let puts = store.puts();
    assert_eq!(puts.last().map(String::as_str), Some(MANIFEST), "the manifest is the last object written: {puts:?}");
    let manifest_at = puts.iter().position(|k| k == MANIFEST).unwrap();
    for (i, key) in puts.iter().enumerate() {
        if key != MANIFEST {
            assert!(i < manifest_at, "{key} was published after the manifest");
        }
    }
    // Everything the manifest references is there, and nothing else is.
    for artifact in &report.manifest.artifacts {
        let key = artifact.url.strip_prefix("https://maps.example/obc/").unwrap();
        assert!(puts.contains(&key.to_string()), "{key} missing from {puts:?}");
        assert!(puts.contains(&format!("{key}.json")), "sidecar for {key} missing");
    }
    assert!(puts.contains(&"presets/default.json".to_string()));
    assert!(!puts.iter().any(|k| k.contains(".bake.json")), "bake state must stay local: {puts:?}");
    assert_eq!(puts.len(), 6, "2 artifacts + 2 sidecars + 1 preset + the manifest: {puts:?}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_artifact_upload_never_swaps_the_manifest_in() {
    let dir = scratch("fail");
    let tree = bake_tree(&dir);
    let store = RecordingStore::failing_on("regions/europe/beta/gamma/default.obcm");

    let err = obc_bake::publish::publish(&tree, &store, &opts(), live()).unwrap_err();
    assert!(err.contains("regions/europe/beta/gamma/default.obcm"), "{err}");
    assert!(!store.puts().contains(&MANIFEST.to_string()), "the previous catalog stays the live one");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_object_that_is_not_fetchable_after_upload_blocks_the_swap() {
    let dir = scratch("vanish");
    let tree = bake_tree(&dir);
    // The upload "succeeded" and the object is not there — a truncated multipart, a
    // bucket policy, a typo'd prefix. The manifest must not go out on top of it.
    let store = RecordingStore::losing("regions/europe/alpha/default.obcm");

    let err = obc_bake::publish::publish(&tree, &store, &opts(), live()).unwrap_err();
    assert!(err.contains("not fetchable"), "{err}");
    assert!(err.contains("refusing to swap the manifest in"), "{err}");
    assert!(!store.puts().contains(&MANIFEST.to_string()));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_directory_publish_produces_a_servable_copy_of_the_tree() {
    let dir = scratch("dir");
    let tree = bake_tree(&dir);
    let dest = dir.join("public");
    let store = DirStore::new(&dest);

    let report = obc_bake::publish::publish(&tree, &store, &opts(), live()).expect("publish");
    assert_eq!(report.objects, 6);

    // Every url in the manifest resolves to a file of exactly the advertised size,
    // which is the property a consumer's `bytes`/`sha256` check depends on.
    for artifact in &report.manifest.artifacts {
        let key = artifact.url.strip_prefix("https://maps.example/obc/").unwrap();
        let path = key.split('/').fold(dest.clone(), |p, s| p.join(s));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), artifact.bytes, "{key}");
    }
    let published = std::fs::read_to_string(dest.join(MANIFEST)).unwrap();
    assert_eq!(published, obc_pack::catalog::manifest_json(&report.manifest), "byte-identical to what was generated");
    assert!(!dest.join("regions/europe/alpha").join(obc_bake::bake::state_file_name("default")).exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_dry_run_generates_the_manifest_and_uploads_nothing() {
    let dir = scratch("dry");
    let tree = bake_tree(&dir);
    let store = RecordingStore::new();

    let report = obc_bake::publish::publish(&tree, &store, &opts(), dry()).expect("dry run");
    assert_eq!(report.objects, 6);
    assert!(store.puts().is_empty());
    // The manifest is still written into the tree — a dry run is how you inspect it.
    assert!(tree.join(MANIFEST).is_file());

    let _ = std::fs::remove_dir_all(&dir);
}

/// The manifest a full bake would have published, as bytes a destination can serve.
fn full_catalog_manifest(dir: &Path) -> String {
    let full = tree_with(&dir.join("full"), &["europe/alpha", "europe/beta/gamma"]);
    let generated = obc_pack::catalog::generate(&full, &opts()).expect("generate");
    obc_pack::catalog::manifest_json(&generated.manifest)
}

#[test]
fn publishing_a_partial_bake_over_a_full_catalog_is_refused() {
    let dir = scratch("shrink");
    let full = full_catalog_manifest(&dir);
    // The CI trap, exactly: a tree holding only some of the regions the live catalog
    // serves — a small-region run, or a `--region`-narrowed one — being published.
    let partial = tree_with(&dir, &["europe/alpha"]);
    let store = RecordingStore::serving(&full);

    let err = obc_bake::publish::publish(&partial, &store, &opts(), live()).unwrap_err();
    assert!(err.contains("would REMOVE"), "{err}");
    assert!(err.contains("europe/beta/gamma [default]"), "the error must name what disappears: {err}");
    assert!(err.contains("--allow-shrink"), "and how to proceed deliberately: {err}");
    // Nothing moved: not the manifest, and not one artifact.
    assert!(store.puts().is_empty(), "a refused publish must not have started uploading: {:?}", store.puts());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_deliberate_shrink_proceeds_loudly_with_allow_shrink() {
    let dir = scratch("shrink-ok");
    let full = full_catalog_manifest(&dir);
    let partial = tree_with(&dir, &["europe/alpha"]);
    let store = RecordingStore::serving(&full);

    let report = obc_bake::publish::publish(
        &partial,
        &store,
        &opts(),
        PublishOptions { allow_shrink: true, ..PublishOptions::default() },
    )
    .expect("--allow-shrink permits it");

    assert_eq!(report.coverage_lost, vec!["europe/beta/gamma [default]".to_string()]);
    assert!(
        report.warnings.iter().any(|w| w.contains("--allow-shrink") && w.contains("europe/beta/gamma")),
        "removing coverage is never silent, even when asked for: {:?}",
        report.warnings
    );
    assert_eq!(store.puts().last().map(String::as_str), Some(MANIFEST), "and it still publishes in order");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_first_publish_ever_has_nothing_to_shrink_from() {
    let dir = scratch("shrink-first");
    let tree = bake_tree(&dir);
    // No manifest at the destination: `get` says None, and a first publish must not
    // need a flag to happen.
    let store = RecordingStore::new();

    let report = obc_bake::publish::publish(&tree, &store, &opts(), live()).expect("first publish");
    assert!(report.coverage_lost.is_empty());
    assert_eq!(store.puts().last().map(String::as_str), Some(MANIFEST));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unreadable_live_manifest_is_not_evidence_of_coverage() {
    let dir = scratch("shrink-garbage");
    let tree = bake_tree(&dir);
    // Something else at that key, or a schema this build does not implement. It
    // cannot be diffed, so it cannot be used to block replacing it.
    for body in ["<html>404</html>", r#"{"schema_version": 99, "artifacts": []}"#] {
        let store = RecordingStore::serving(body);
        let report = obc_bake::publish::publish(&tree, &store, &opts(), live()).expect("publish proceeds");
        assert!(report.coverage_lost.is_empty());
        assert_eq!(store.puts().last().map(String::as_str), Some(MANIFEST));
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn growing_the_catalog_and_republishing_the_same_one_are_both_fine() {
    let dir = scratch("shrink-grow");
    let one = tree_with(&dir.join("one"), &["europe/alpha"]);
    let live_manifest =
        obc_pack::catalog::manifest_json(&obc_pack::catalog::generate(&one, &opts()).expect("generate").manifest);

    // Same set → no loss.
    let store = RecordingStore::serving(&live_manifest);
    let report = obc_bake::publish::publish(&one, &store, &opts(), live()).expect("republish");
    assert!(report.coverage_lost.is_empty());

    // Superset → no loss either; the guard is about removal, not about change.
    let two = tree_with(&dir, &["europe/alpha", "europe/beta/gamma"]);
    let store = RecordingStore::serving(&live_manifest);
    let report = obc_bake::publish::publish(&two, &store, &opts(), live()).expect("growing publish");
    assert!(report.coverage_lost.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_dir_publish_diffs_against_the_manifest_already_in_that_directory() {
    let dir = scratch("shrink-dir");
    let dest = dir.join("public");
    // A full publish, then a partial one over the same directory — the offline
    // workstation flow, and the guard works there with no network at all.
    let full = tree_with(&dir.join("full"), &["europe/alpha", "europe/beta/gamma"]);
    obc_bake::publish::publish(&full, &DirStore::new(&dest), &opts(), live()).expect("full publish");

    let partial = tree_with(&dir, &["europe/alpha"]);
    let err = obc_bake::publish::publish(&partial, &DirStore::new(&dest), &opts(), live()).unwrap_err();
    assert!(err.contains("europe/beta/gamma [default]"), "{err}");

    // The live directory is untouched: the old manifest and both artifacts remain.
    let served = std::fs::read_to_string(dest.join(MANIFEST)).unwrap();
    assert!(served.contains("europe/beta/gamma"), "the previous catalog is still the live one");
    assert!(dest.join("regions/europe/beta/gamma/default.obcm").is_file());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_dry_run_never_asks_the_destination_anything() {
    let dir = scratch("shrink-dry");
    let full = full_catalog_manifest(&dir);
    let partial = tree_with(&dir, &["europe/alpha"]);
    let store = RecordingStore::serving(&full);

    // A dry run moves no bytes, so it cannot shrink anything — it must not fail on a
    // tree a real publish would refuse, and it must not need the destination at all.
    let report = obc_bake::publish::publish(&partial, &store, &opts(), dry()).expect("dry run of a partial tree");
    assert!(report.coverage_lost.is_empty());
    assert!(store.puts().is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_plan_puts_the_manifest_last_by_construction() {
    let dir = scratch("plan");
    let tree = bake_tree(&dir);
    obc_pack::catalog::write_atomic(
        &tree.join(MANIFEST),
        &obc_pack::catalog::generate(&tree, &opts()).unwrap().manifest,
    )
    .unwrap();

    let plan = obc_bake::publish::plan(&tree).unwrap();
    assert_eq!(plan.last().unwrap().kind, ObjectKind::Manifest);
    assert_eq!(plan.iter().filter(|o| o.kind == ObjectKind::Manifest).count(), 1);
    // Sorted keys everywhere else, so a publish log diffs against the previous one.
    let keys: Vec<&str> = plan[..plan.len() - 1].iter().map(|o| o.key.as_str()).collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(keys, sorted);

    let _ = std::fs::remove_dir_all(&dir);
}
