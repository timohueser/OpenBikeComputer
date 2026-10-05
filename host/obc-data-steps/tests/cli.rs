//! The `obc data` binary: what a command writes and its exit status. A temporary directory stands
//! in for the store.

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

/// `obc data ARGS` in the repository, with the store in `temp`.
fn obc_data(temp: &Temp, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_obc-data"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("OBC_DATA_STORE", temp.0.join("store"))
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn a_plan_of_live_without_products_is_empty_and_a_build_refuses_another_plan() {
    let temp = Temp::new("plan");
    let out = obc_data(&temp, &["plan", "live", "--json"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let mut plan: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(plan["env"], "live");
    assert_eq!(plan["groups"], serde_json::json!([]));

    plan["groups"] = serde_json::json!([{"id": "planner/routing", "fetches": [], "builds": []}]);
    let file = temp.0.join("plan.json");
    std::fs::write(&file, plan.to_string()).unwrap();
    let out = obc_data(&temp, &["build", "live", "--plan", file.to_str().unwrap(), "--json"]);
    assert_eq!(out.status.code(), Some(3), "{}", String::from_utf8_lossy(&out.stderr));
    let error: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(error["error"]["code"], "plan_outdated");
}
