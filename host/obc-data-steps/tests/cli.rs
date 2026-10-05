//! The `obc data` binary: what a command writes and its exit status. A temporary directory stands
//! in for the store.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use obc_data::env::Env;
use obc_data::regions::Regions;
use obc_data::sources::Registry;
use obc_data::store::{sha256_hex, write_atomic, FileRecord, Requested, Snapshot, Store};

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

/// The store of `temp`, with the `.poly` of the live region as its fetch gives it: a box around
/// Freiburg, so the plan needs no network.
fn with_live_outline(temp: &Temp) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let (sources, regions) = (Registry::load(&root).unwrap().sources, Regions::load(&root).unwrap());
    let version = Env::load(&root, "live", &sources, &regions).unwrap().version("geofabrik-poly").unwrap().to_string();
    let store = Store::at(temp.0.join("store"));
    let poly = "box\n1\n   7.77 47.97\n   7.93 47.97\n   7.93 48.14\n   7.77 48.14\n   7.77 47.97\nEND\nEND\n";
    let (file, sha256) = (store.partial("poly"), sha256_hex(poly.as_bytes()));
    write_atomic(&file, poly.as_bytes()).unwrap();
    store.insert(&file, &sha256).unwrap();
    let (name, size) = ("europe/germany/baden-wuerttemberg.poly".to_string(), poly.len() as u64);
    let url = format!("https://download.geofabrik.de/{name}");
    let record = FileRecord { name: name.clone(), url, size, sha256, retrieved: String::new() };
    let files = vec![record];
    store.put_snapshot(&Snapshot { source: "geofabrik-poly".into(), version: version.clone(), files }).unwrap();
    let params = vec![("area".into(), "europe/germany/baden-wuerttemberg".into())];
    store.put_requested("geofabrik-poly", &Requested { version, params, files: vec![name] }).unwrap();
}

#[test]
fn a_plan_of_live_builds_the_terrain_of_each_leaf_and_a_build_refuses_another_plan() {
    let temp = Temp::new("plan");
    with_live_outline(&temp);
    let out = obc_data(&temp, &["plan", "live", "--json"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let mut plan: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(plan["env"], "live");
    let ids: Vec<&str> = plan["groups"].as_array().unwrap().iter().map(|group| group["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["maps/terrain/0037-0032"]);
    assert_eq!(plan["groups"][0]["fetches"][0]["source"], "copernicus-glo-30");

    plan["groups"] = serde_json::json!([{"id": "planner/routing", "fetches": [], "builds": []}]);
    let file = temp.0.join("plan.json");
    std::fs::write(&file, plan.to_string()).unwrap();
    let out = obc_data(&temp, &["build", "live", "--plan", file.to_str().unwrap(), "--json"]);
    assert_eq!(out.status.code(), Some(3), "{}", String::from_utf8_lossy(&out.stderr));
    let error: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(error["error"]["code"], "plan_outdated");
}
