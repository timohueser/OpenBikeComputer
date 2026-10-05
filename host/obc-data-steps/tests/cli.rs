//! The `obc data` binary: what a command writes and its exit status. A temporary directory stands
//! in for the store, and rclone's `local` backend for the bucket.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use obc_data::r2::{Bucket, Put, Upload, REMOVAL_LOG};

struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("obc-data-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bucket")).unwrap();
        Self(dir)
    }

    fn bucket(&self) -> PathBuf {
        self.0.join("bucket")
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn put(bucket: &Bucket, file: &Path, key: &str) {
    assert_eq!(bucket.put(file, key, &Upload::default()).unwrap(), Put::Uploaded);
}

/// `obc data ARGS` in the repository, with the store and the bucket in `temp`.
fn obc_data(temp: &Temp, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_obc-data"));
    // A credential from tools/obc.local must neither reach the child nor clash with the local bucket.
    for (name, _) in std::env::vars_os() {
        let text = name.to_string_lossy();
        if text.starts_with("OBC_R2_") || text.starts_with("OBC_FIXTURE_R2_") {
            command.env_remove(&name);
        }
    }
    command
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("OBC_R2_LOCAL_DIR", temp.bucket())
        .env("OBC_DATA_STORE", temp.0.join("store"))
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn obc_data_r2(temp: &Temp, args: &[&str]) -> Output {
    obc_data(temp, &[&["r2"], args].concat())
}

#[test]
fn a_plan_of_live_without_products_is_empty_and_a_build_refuses_another_plan() {
    let temp = Temp::new("plan");
    let out = obc_data(&temp, &["plan", "live", "--json"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let mut plan: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(plan["env"], "live");
    assert_eq!(plan["groups"], serde_json::json!([]));

    plan["groups"] = serde_json::json!([{"id": "planner/routing", "blocked": null, "fetches": [], "builds": []}]);
    let file = temp.file("plan.json", plan.to_string().as_bytes());
    let out = obc_data(&temp, &["build", "live", "--plan", file.to_str().unwrap(), "--json"]);
    assert_eq!(out.status.code(), Some(3), "{}", String::from_utf8_lossy(&out.stderr));
    let error: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(error["error"]["code"], "plan_outdated");
}

#[test]
fn delete_without_a_terminal_needs_yes_and_changes_nothing() {
    let temp = Temp::new("refuse");
    let bucket = Bucket::local(&temp.bucket());
    put(&bucket, &temp.file("stray", b"stray"), "uploads/stray.obcm");

    let out = obc_data_r2(&temp, &["delete", "uploads/stray.obcm", "--reason", "a stray upload"]);
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("uploads/stray.obcm"), "the plan is shown");

    // With `--json`, standard output is only the error, also for an argument that clap refuses.
    for (args, code) in [
        (&["delete", "uploads/stray.obcm", "--reason", "a stray upload", "--json"][..], "no_terminal"),
        (&["delete", "uploads/stray.obcm", "--json"], "usage"),
    ] {
        let out = obc_data_r2(&temp, args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let error: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(error["error"]["code"], code, "{error}");
        assert!(!error["error"]["message"].as_str().unwrap().is_empty() && error["error"]["fix"].is_string());
    }
    assert!(temp.bucket().join("uploads/stray.obcm").exists());
    assert!(!temp.bucket().join(REMOVAL_LOG).exists());
}

#[test]
fn delete_with_yes_appends_to_the_removal_log_then_deletes() {
    let temp = Temp::new("delete");
    let bucket = Bucket::local(&temp.bucket());
    put(&bucket, &temp.file("stray", b"stray"), "uploads/stray.obcm");
    put(&bucket, &temp.file("keep", b"keep"), "uploads/keep.obcm");
    put(&bucket, &temp.file("log", b"{\"key\": \"uploads/old.obcm\"}\n"), REMOVAL_LOG);

    for refused in [
        &["delete", "uploads/stray.obcm", "uploads/gone", "--reason", "x", "--yes"][..],
        &["delete", "uploads/stray.obcm", REMOVAL_LOG, "--reason", "x", "--yes"],
        &["delete", "--prefix", "uploads/stray.obcm", "--reason", "x", "--yes"],
    ] {
        assert_eq!(obc_data_r2(&temp, refused).status.code(), Some(1), "{refused:?}");
        assert!(temp.bucket().join("uploads/stray.obcm").exists(), "{refused:?} deletes nothing");
        assert!(temp.bucket().join(REMOVAL_LOG).exists());
    }

    let out = obc_data_r2(&temp, &["delete", "uploads/stray.obcm", "--reason", "a stray upload", "--yes"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!temp.bucket().join("uploads/stray.obcm").exists());
    assert!(temp.bucket().join("uploads/keep.obcm").exists());

    let log = std::fs::read_to_string(temp.bucket().join(REMOVAL_LOG)).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines[0], "{\"key\": \"uploads/old.obcm\"}", "the history is appended to");
    let record: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(record["key"], "uploads/stray.obcm");
    assert_eq!(record["bytes"], 5);
    assert_eq!(record["reason"], "a stray upload");
    assert!(record["removed"].as_str().unwrap().ends_with('Z') && record["by"].is_string());
}
