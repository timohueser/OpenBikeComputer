//! The `obc data` binary: what a command writes and its exit status. Temporary directories stand in
//! for the store and for R2.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("obc-data-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `obc data ARGS` in the repository, with the store in `temp` and an empty bucket: nothing is
/// live.
fn obc_data(temp: &Temp, args: &[&str]) -> Output {
    let bucket = temp.0.join("bucket");
    std::fs::create_dir_all(&bucket).unwrap();
    Command::new(env!("CARGO_BIN_EXE_obc-data"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("OBC_DATA_STORE", temp.0.join("store"))
        .env("OBC_R2_LOCAL_DIR", bucket)
        .env_remove("OBC_R2_BUCKET")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn live_leaves_out_each_product_without_a_client_document() {
    let temp = Temp::new("blocked");
    let out = obc_data(&temp, &["plan", "live", "--json"]);
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
    let plan: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let blocked: Vec<&str> =
        plan["blocked"].as_array().unwrap().iter().map(|blocked| blocked["product"].as_str().unwrap()).collect();
    assert_eq!(blocked, ["maps", "planner"], "neither product has a pointer yet");
    assert_eq!(plan["groups"], serde_json::json!([]));

    let out = obc_data(&temp, &["build", "live", "--json"]);
    assert_eq!(out.status.code(), Some(4), "{}", String::from_utf8_lossy(&out.stderr));
    let error: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(error["error"]["code"], "blocked");
}
