//! The CLI, through the real binary: the flags a workflow types are as much a
//! contract as the library API, and `.github/workflows/bake.yml` is the caller that
//! cannot be unit-tested.
//!
//! Offline: `--source` points at a directory of fixture extracts, and the publish
//! step is a dry run.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("obc-bake-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn obc_bake() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_obc-bake"));
    // A developer's shell may have one; these tests decide their own.
    cmd.env_remove("OBC_CATALOG_URL");
    cmd
}

#[test]
fn regions_lists_the_curated_shelf() {
    let out = obc_bake().arg("regions").output().expect("run");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("europe/germany/bayern"), "{text}");
    assert!(text.contains("19 regions"), "{text}");
}

#[test]
fn bake_then_publish_is_the_whole_loop() {
    let dir = scratch("loop");
    let extracts = dir.join("extracts/europe");
    std::fs::create_dir_all(&extracts).unwrap();
    std::fs::copy(repo("builder/tests/corpus/data/tiny.osm.pbf"), extracts.join("testland-latest.osm.pbf")).unwrap();
    let regions = dir.join("regions.toml");
    std::fs::write(&regions, "regions = [ { id = \"europe/testland\", name = \"Testland\" } ]\n").unwrap();
    let tree = dir.join("tree");

    let out = obc_bake()
        .args(["bake", "--out"])
        .arg(&tree)
        .arg("--regions")
        .arg(&regions)
        .arg("--presets-dir")
        .arg(repo("builder/presets"))
        .args(["--source"])
        .arg(dir.join("extracts"))
        .arg("--no-land")
        .output()
        .expect("run bake");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{log}");
    assert!(log.contains("bake summary"), "{log}");
    assert!(tree.join("regions/europe/testland/bikepacking.obcm").is_file(), "{log}");

    // A second run says so rather than re-packing.
    let out = obc_bake()
        .args(["bake", "--out"])
        .arg(&tree)
        .arg("--regions")
        .arg(&regions)
        .arg("--presets-dir")
        .arg(repo("builder/presets"))
        .args(["--source"])
        .arg(dir.join("extracts"))
        .arg("--no-land")
        .output()
        .expect("run bake again");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("unchanged"), "the second run must skip");

    let out = obc_bake()
        .arg("publish")
        .arg(&tree)
        .args(["--base-url", "https://maps.example/obc", "--dry-run", "--generated-at", "2026-07-29T00:00:00Z"])
        .output()
        .expect("run publish");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{log}");
    assert!(log.contains("nothing uploaded"), "{log}");
    let manifest = std::fs::read_to_string(tree.join("catalog.json")).expect("manifest written");
    assert!(manifest.contains("\"region_id\": \"europe/testland\""), "{manifest}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn publishing_a_shrunken_catalog_is_refused_at_the_command_line() {
    let dir = scratch("shrink");
    let extracts = dir.join("extracts/europe");
    std::fs::create_dir_all(&extracts).unwrap();
    for region in ["alpha", "beta"] {
        std::fs::copy(
            repo("builder/tests/corpus/data/tiny.osm.pbf"),
            extracts.join(format!("{region}-latest.osm.pbf")),
        )
        .unwrap();
    }
    let dest = dir.join("public");

    let bake = |tree: &std::path::Path, regions: &str| {
        let list = tree.with_extension("toml");
        std::fs::write(&list, regions).unwrap();
        let out = obc_bake()
            .args(["bake", "--out"])
            .arg(tree)
            .arg("--regions")
            .arg(&list)
            .arg("--presets-dir")
            .arg(repo("builder/presets"))
            .args(["--source"])
            .arg(dir.join("extracts"))
            .arg("--no-land")
            .output()
            .expect("run bake");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    };
    let publish = |tree: &std::path::Path, extra: &[&str]| {
        obc_bake()
            .arg("publish")
            .arg(tree)
            .args(["--base-url", "https://maps.example/obc", "--target"])
            .arg(format!("dir:{}", dest.display()))
            .args(extra)
            .output()
            .expect("run publish")
    };

    let full = dir.join("full-tree");
    bake(
        &full,
        "regions = [ { id = \"europe/alpha\", name = \"Alpha\" }, { id = \"europe/beta\", name = \"Beta\" } ]\n",
    );
    assert!(publish(&full, &[]).status.success(), "the first publish establishes the catalog");

    // The trap: a narrower tree published over it.
    let partial = dir.join("partial-tree");
    bake(&partial, "regions = [ { id = \"europe/alpha\", name = \"Alpha\" } ]\n");
    let out = publish(&partial, &[]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "a shrinking publish must fail: {err}");
    assert!(err.contains("europe/beta [bikepacking]"), "{err}");

    // …and `--allow-shrink` parses as a switch (not as a value flag swallowing the
    // next argument) and lets the deliberate case through, loudly.
    let out = publish(&partial, &["--allow-shrink"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("COVERAGE REMOVED"), "{err}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_region_outside_the_curated_list_is_refused() {
    let out = obc_bake().args(["bake", "--out", "/tmp/nope", "--region", "europe/france"]).output().expect("run");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not in the curated region list"), "{err}");
}

/// The two style-selection flags #1036 retired are refused **on both paths**.
///
/// `--preset` was the v1 spelling and `--schema-preset` the cell path's (#1025), and
/// the callers that pass them are scripts and workflow inputs — the readers least
/// likely to notice a flag that quietly stopped meaning anything. The `--cells` case
/// is the one that regressed once already: the guard sat *below* the branch, so the
/// path that had its own retired flag was the path with no guard at all.
#[test]
fn the_retired_style_flags_are_refused_on_both_bake_paths() {
    for (flag, path) in [
        ("--preset", &[][..]),
        ("--schema-preset", &[][..]),
        ("--preset", &["--cells", "--base-url", "https://maps.example/obc"][..]),
        ("--schema-preset", &["--cells", "--base-url", "https://maps.example/obc"][..]),
    ] {
        let out = obc_bake()
            .args(["bake", "--out", "/tmp/nope"])
            .args(path)
            .args([flag, "high-detail"])
            .output()
            .expect("run");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "`{flag}` {path:?} must not be swallowed: {err}");
        assert!(err.contains(&format!("`{flag}` retired with the preset shelf")), "{err}");
        assert!(err.contains("--skin ID"), "the error has to say what to use instead: {err}");
    }
}

#[test]
fn the_version_guard_skips_gracefully_with_no_catalog_url() {
    let out = obc_bake().arg("check-obcm-version").output().expect("run");
    assert!(out.status.success(), "an unconfigured guard must not be a red check");
    assert!(String::from_utf8_lossy(&out.stdout).contains("skipped"));
}

#[test]
fn the_version_guard_fails_on_a_catalog_it_cannot_read() {
    // "Configured but unreachable" is exactly when a guard has to speak up. Port 1 on
    // loopback refuses immediately — no DNS, no network, no timeout.
    let out = obc_bake()
        .args(["check-obcm-version", "--catalog-url", "http://127.0.0.1:1/catalog.json"])
        .output()
        .expect("run");
    assert!(!out.status.success());
}
