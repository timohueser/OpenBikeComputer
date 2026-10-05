//! The R2 client against rclone's `local` backend: a temporary directory stands in for the bucket.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use obc_data::r2::{Bucket, Put, Upload, REMOVAL_LOG};

struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("obc-data-r2-{name}-{}", std::process::id()));
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

#[test]
fn verify_detects_a_mismatch_and_an_immutable_key_keeps_its_bytes() {
    let temp = Temp::new("verify");
    let bucket = Bucket::local(&temp.bucket());
    let original = temp.file("original", b"cell bytes");
    let same_size = temp.file("same-size", b"cell bytez");
    put(&bucket, &original, "cells/a.obcm");
    bucket.verify(&original, "cells/a.obcm").unwrap();

    let error = bucket.verify(&same_size, "cells/a.obcm").unwrap_err();
    assert!(error.contains("MD5"), "{error}");
    let error = bucket.verify(&temp.file("short", b"cell"), "cells/a.obcm").unwrap_err();
    assert!(error.contains("holds 10 bytes"), "{error}");
    assert!(bucket.verify(&original, "cells/b.obcm").unwrap_err().contains("not in"));

    let immutable = Upload { immutable: true, ..Upload::default() };
    assert!(bucket.put(&same_size, "cells/a.obcm", &immutable).unwrap_err().contains("immutable"));
    assert_eq!(bucket.put(&original, "cells/a.obcm", &immutable).unwrap(), Put::AlreadyThere);
    assert_eq!(std::fs::read(temp.bucket().join("cells/a.obcm")).unwrap(), b"cell bytes");
}

#[test]
fn stat_tells_an_absent_object_from_an_empty_one() {
    let temp = Temp::new("stat");
    let bucket = Bucket::local(&temp.bucket());
    put(&bucket, &temp.file("empty", b""), "cells/empty.obcm");
    let keys = ["cells/empty.obcm".to_string(), "cells/never.obcm".to_string()];
    let found = bucket.stat(&keys).unwrap();
    assert_eq!(found.keys().collect::<Vec<_>>(), ["cells/empty.obcm"]);
    assert_eq!(found["cells/empty.obcm"].bytes, 0);
    assert!(bucket.list("nothing/here").unwrap().is_empty());
}

fn obc_data_r2(temp: &Temp, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_obc-data"))
        .arg("r2")
        .args(args)
        .env("OBC_R2_LOCAL_DIR", temp.bucket())
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn delete_without_a_terminal_needs_yes_and_changes_nothing() {
    let temp = Temp::new("refuse");
    let bucket = Bucket::local(&temp.bucket());
    put(&bucket, &temp.file("stray", b"stray"), "uploads/stray.obcm");

    let out = obc_data_r2(&temp, &["delete", "uploads/stray.obcm", "--reason", "a stray upload"]);
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("uploads/stray.obcm"), "the plan is shown");
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
