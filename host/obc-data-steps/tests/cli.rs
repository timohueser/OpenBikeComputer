//! The `obc data` binary: what a command writes and its exit status. Temporary directories stand in
//! for the store and for R2.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

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

/// Record `text` as the file `name` of `source@version`, as a fetch with `params` gives it.
fn fetched(store: &Store, source: &str, version: &str, params: Vec<(String, String)>, name: &str, text: &str) {
    let (file, sha256) = (store.partial(name.rsplit('/').next().unwrap()), sha256_hex(text.as_bytes()));
    write_atomic(&file, text.as_bytes()).unwrap();
    store.insert(&file, &sha256).unwrap();
    let (url, size) = (format!("https://example.org/{name}"), text.len() as u64);
    let files = vec![FileRecord { name: name.into(), url, size, sha256, retrieved: String::new() }];
    store.put_snapshot(&Snapshot { source: source.into(), version: version.into(), files }).unwrap();
    if !params.is_empty() {
        store.put_requested(source, &Requested { version: version.into(), params, files: vec![name.into()] }).unwrap();
    }
}

/// The store of `temp`, with what the step lists of live read, so the plan needs no network: the
/// `.poly` of the live region is a box around Freiburg, and its extract is a stand-in.
fn with_live_outline(temp: &Temp) {
    let store = Store::at(temp.0.join("store"));
    let poly = "box\n1\n   7.77 47.97\n   7.93 47.97\n   7.93 48.14\n   7.77 48.14\n   7.77 47.97\nEND\nEND\n";
    let area = vec![("area".into(), "europe/germany/baden-wuerttemberg".into())];
    fetched(&store, "geofabrik-poly", "2026-10-05", area.clone(), "europe/germany/baden-wuerttemberg.poly", poly);
    fetched(&store, "geofabrik-extracts", "2026-10-05", area, "europe/germany/baden-wuerttemberg.osm.pbf", "osm");
    fetched(&store, "land-polygons", "2026-10-05", Vec::new(), "land-polygons-split-3857.zip", "land");
    let tiles = "Copernicus_DSM_COG_10_N47_00_E007_00_DEM\nCopernicus_DSM_COG_10_N48_00_E007_00_DEM\n";
    fetched(&store, "copernicus-glo-30-tiles", "2022-05-09", Vec::new(), "tileList.txt", tiles);
}

#[test]
fn a_plan_of_live_builds_the_layers_of_each_product_and_a_build_refuses_another_plan() {
    let temp = Temp::new("plan");
    with_live_outline(&temp);
    let out = obc_data(&temp, &["plan", "live", "--json"]);
    assert_eq!(out.status.code(), Some(4), "nothing live and nothing stored names the manual GLO-30 tiles");
    let error: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(error["error"]["fix"], "Plan with `--move copernicus-glo-30@VERSION`.");

    // Every manual source moves to a version, so the plan fetches nothing.
    let moves = [
        ("copernicus-glo-30", "2022-05-09"),
        ("protomaps-assets", "028c18f713baecad011301ff7a69acc39bcc2ae7"),
        ("tangrams-icons", "92510779634f4a006c61ea70e50cb8c52c765a81"),
        ("query-model", "spike/query-parser-v2"),
        ("nominatim-country-data", "5.3.2"),
    ];
    let mut args = vec!["plan".to_string(), "live".into(), "--json".into()];
    args.extend(moves.iter().flat_map(|(source, version)| ["--move".into(), format!("{source}@{version}")]));
    let out = obc_data(&temp, &args.iter().map(String::as_str).collect::<Vec<_>>());
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
    let mut plan: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(plan["env"], "live");
    let moved: serde_json::Map<_, _> =
        moves.iter().map(|(source, version)| (source.to_string(), (*version).into())).collect();
    assert_eq!(plan["moves"], serde_json::Value::Object(moved));
    let ids: Vec<&str> = plan["groups"].as_array().unwrap().iter().map(|group| group["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["maps/terrain/0037-0032", "planner/osm", "planner/terrain", "planner/assets", "planner/model"]);
    assert_eq!(plan["groups"][0]["fetches"][0]["source"], "copernicus-glo-30");

    plan["groups"] = serde_json::json!([{"id": "planner/routing", "fetches": [], "builds": []}]);
    let file = temp.0.join("plan.json");
    std::fs::write(&file, plan.to_string()).unwrap();
    let out = obc_data(&temp, &["build", "live", "--plan", file.to_str().unwrap(), "--json"]);
    assert_eq!(out.status.code(), Some(3), "{}", String::from_utf8_lossy(&out.stderr));
    let error: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(error["error"]["code"], "plan_outdated");
}
