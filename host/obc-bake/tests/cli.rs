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
fn regions_lists_the_curated_coverage() {
    let out = obc_bake().arg("regions").output().expect("run");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("europe/germany/bayern"), "{text}");
    assert!(text.contains("20 regions"), "{text}");
}

#[test]
fn bake_then_publish_is_the_whole_loop() {
    let dir = scratch("loop");
    let extracts = dir.join("extracts/europe");
    std::fs::create_dir_all(&extracts).unwrap();
    std::fs::copy(repo("builder/tests/corpus/data/tiny.osm.pbf"), extracts.join("testland-latest.osm.pbf")).unwrap();
    std::fs::write(
        extracts.join("testland.poly"),
        "testland\n1\n  7.790 47.980\n  7.830 47.980\n  7.830 48.010\n  7.790 48.010\n  7.790 47.980\nEND\nEND\n",
    )
    .unwrap();
    let regions = dir.join("regions.toml");
    std::fs::write(&regions, "regions = [ { id = \"europe/testland\", name = \"Testland\" } ]\n").unwrap();
    let tree = dir.join("tree");

    let out = obc_bake()
        .args(["bake", "--out"])
        .arg(&tree)
        .args(["--base-url", "https://maps.example/obc"])
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
    assert!(log.contains("cell bake summary"), "{log}");
    assert!(tree.join("catalog.json").is_file(), "{log}");
    assert!(tree.join("regions/europe/testland/cells.json").is_file(), "{log}");

    // A second run says so rather than re-packing.
    let out = obc_bake()
        .args(["bake", "--out"])
        .arg(&tree)
        .args(["--base-url", "https://maps.example/obc"])
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
    assert!(manifest.contains("\"id\": \"europe/testland\""), "{manifest}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_region_outside_the_curated_list_is_refused() {
    let out = obc_bake().args(["bake", "--out", "/tmp/nope", "europe/france"]).output().expect("run");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not in the curated region list"), "{err}");
}

#[test]
fn all_checks_osmium_before_touching_the_planet_source() {
    let out = obc_bake()
        .env("OBC_OSMIUM", "/definitely/not/an/osmium-binary")
        .args(["bake", "--all", "--source", "http://127.0.0.1:1/planet.osm.pbf", "--presets-dir"])
        .arg(repo("builder/presets"))
        .output()
        .expect("run");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("is required for `obc bake --all`"), "{err}");
    assert!(!err.contains("HEAD http"), "the prerequisite check must happen before network I/O: {err}");
}

#[test]
fn all_checks_replication_tool_before_touching_the_planet_source() {
    let out = obc_bake()
        .env("OBC_OSMIUM", "true")
        .env("OBC_PYOSMIUM_UP_TO_DATE", "/definitely/not/a/pyosmium-up-to-date-binary")
        .args(["bake", "--all", "--source", "http://127.0.0.1:1/planet.osm.pbf", "--presets-dir"])
        .arg(repo("builder/presets"))
        .output()
        .expect("run");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("obc doctor --install"), "{err}");
    assert!(!err.contains("HEAD http"), "the prerequisite check must happen before network I/O: {err}");
}

#[test]
fn all_and_a_curated_selector_are_mutually_exclusive() {
    let out = obc_bake().args(["bake", "--all", "europe/germany"]).output().expect("run");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot be combined with region selectors"));
}

#[test]
fn unknown_flags_are_refused_instead_of_ignored() {
    let out = obc_bake().args(["bake", "--out", "/tmp/nope", "--typo", "value"]).output().expect("run");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "an unknown flag must not be swallowed: {err}");
    assert!(err.contains("unknown flag `--typo`"), "{err}");
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
