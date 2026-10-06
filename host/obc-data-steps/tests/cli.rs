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
fn a_private_worker_cannot_dispatch_cli_or_capture_callbacks_without_the_launcher() {
    for args in [vec!["--help"], vec!["landmark-candidates", "--help"]] {
        let out = Command::new(env!("CARGO_BIN_EXE_obc-data-worker"))
            .args(args)
            .env_remove(obc_data::worker::ROOT)
            .env_remove(obc_data::worker::CODE)
            .env_remove(obc_data::worker::EXE)
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        assert!(String::from_utf8_lossy(&out.stderr).contains("start this worker through obc data"));
    }
    for binary in [env!("CARGO_BIN_EXE_obc-data"), env!("CARGO_BIN_EXE_obc-data-worker")] {
        let temp = Temp::new(if binary.ends_with("worker") { "worker-json" } else { "launcher-json" });
        let out = Command::new(binary)
            .args(["status", "--json"])
            .current_dir(&temp.0)
            .env_remove(obc_data::worker::ROOT)
            .env_remove(obc_data::worker::CODE)
            .env_remove(obc_data::worker::EXE)
            .output()
            .unwrap();
        assert!(!out.status.success());
        let response: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(response["error"]["code"], "failed");
    }
}

#[test]
fn an_ordinary_plan_reports_unprepared_maps_without_fetching_bulk_data() {
    let temp = Temp::new("blocked");
    let store = obc_data::store::Store::at(temp.0.join("store"));
    let region = "europe/germany/baden-wuerttemberg";
    let index = serde_json::json!({"type":"FeatureCollection","features":[{"type":"Feature",
        "properties":{"id":"bw","name":"Baden-Württemberg","parent":null,"urls":{"pbf":format!("https://download.geofabrik.de/{region}-latest.osm.pbf")}},
        "geometry":{"type":"Polygon","coordinates":[[[7.79,47.99],[7.82,47.99],[7.82,48.02],[7.79,48.02],[7.79,47.99]]]}}]});
    for (source, name, body, params) in [
        (
            "geofabrik-poly",
            format!("{region}.poly"),
            "box\n1\n7.79 47.99\n7.82 47.99\n7.82 48.02\n7.79 48.02\n7.79 47.99\nEND\nEND\n".to_string(),
            vec![("area".into(), region.into())],
        ),
        ("copernicus-glo-30-tiles", "tileList.txt".into(), String::new(), Vec::new()),
        ("geofabrik-index", "index-v1.json".into(), index.to_string(), Vec::new()),
    ] {
        let file = store.partial(&name);
        obc_data::store::write_atomic(&file, body.as_bytes()).unwrap();
        let sha256 = obc_data::store::sha256_hex(body.as_bytes());
        store.insert(&file, &sha256).unwrap();
        let version = "2026-10-06".to_string();
        store
            .put_snapshot(&obc_data::store::Snapshot {
                source: source.into(),
                version: version.clone(),
                files: vec![obc_data::store::FileRecord {
                    name: name.clone(),
                    sha256,
                    size: body.len() as u64,
                    url: format!("https://example.org/{name}"),
                    retrieved: String::new(),
                }],
            })
            .unwrap();
        if !params.is_empty() {
            store.put_requested(source, &obc_data::store::Requested { version, params, files: vec![name] }).unwrap();
        }
    }
    let out = obc_data(&temp, &["plan", "live", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let response: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(response["needs_prepare"], true);
    assert!(response["blocked"].as_array().unwrap().iter().any(|product| {
        product["product"] == "maps" && product["reason"].as_str().unwrap().contains("do not fetch bulk data")
    }));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("fetching"));

    for source in ["geofabrik-extracts", "land-polygons", "copernicus-glo-30"] {
        assert!(store.snapshot(source, "2026-10-06").unwrap().is_none());
    }
}
