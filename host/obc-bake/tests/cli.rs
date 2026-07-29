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
        .args(["--preset", "default", "--source"])
        .arg(dir.join("extracts"))
        .arg("--no-land")
        .output()
        .expect("run bake");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{log}");
    assert!(log.contains("bake summary"), "{log}");
    assert!(tree.join("regions/europe/testland/default.obcm").is_file(), "{log}");

    // A second run says so rather than re-packing.
    let out = obc_bake()
        .args(["bake", "--out"])
        .arg(&tree)
        .arg("--regions")
        .arg(&regions)
        .arg("--presets-dir")
        .arg(repo("builder/presets"))
        .args(["--preset", "default", "--source"])
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
fn a_region_outside_the_curated_list_is_refused() {
    let out = obc_bake().args(["bake", "--out", "/tmp/nope", "--region", "europe/france"]).output().expect("run");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not in the curated region list"), "{err}");
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
