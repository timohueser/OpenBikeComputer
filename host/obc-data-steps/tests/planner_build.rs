//! The planner layers of `obc data build` over a road grid of nine junctions near Freiburg
//! (`data/planner.osm`): the routing package verifies, and a second plan builds nothing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use obc_data::engine::plan::plan;
use obc_data::engine::release::release;
use obc_data::engine::runs::{Context, Limits, Run};
use obc_data::env::Env;
use obc_data::fetch::http::Http;
use obc_data::product::Product;
use obc_data::regions::{parse_region, Regions};
use obc_data::store::{sha256_hex, write_atomic, FileRecord, Requested, Snapshot, Store};
use obc_data_steps::maps::TILE_LIST;
use obc_data_steps::planner::Planner;
use obc_dem::step::GLO30;

const AREA: &str = "europe/test";
const DAY: &str = "2026-10-01";

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Record `bytes` as the file `name` of `source@version`, as a fetch with `params` gives it.
fn fetched(store: &Store, source: &str, version: &str, params: Vec<(String, String)>, name: &str, bytes: &[u8]) {
    let (file, sha256) = (store.partial("file"), sha256_hex(bytes));
    write_atomic(&file, bytes).unwrap();
    store.insert(&file, &sha256).unwrap();
    let (url, size) = (format!("https://example.org/{name}"), bytes.len() as u64);
    let files = vec![FileRecord { name: name.into(), url, size, sha256, retrieved: String::new() }];
    store.put_snapshot(&Snapshot { source: source.into(), version: version.into(), files }).unwrap();
    if !params.is_empty() {
        store.put_requested(source, &Requested { version: version.into(), params, files: vec![name.into()] }).unwrap();
    }
}

#[test]
fn a_build_makes_a_routing_package_that_verifies_and_a_second_plan_builds_nothing() {
    let temp = Temp(std::env::temp_dir().join(format!("obc-data-planner-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&temp.0);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store = Store::at(temp.0.join("store"));
    let area = || vec![("area".to_string(), AREA.to_string())];
    let poly = "test\n1\n   7.79 47.99\n   7.82 47.99\n   7.82 48.02\n   7.79 48.02\n   7.79 47.99\nEND\nEND\n";
    fetched(&store, "geofabrik-poly", DAY, area(), &format!("{AREA}.poly"), poly.as_bytes());
    let pbf = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/planner.osm.pbf")).unwrap();
    fetched(&store, "geofabrik-extracts", DAY, area(), &format!("{AREA}-261001.osm.pbf"), &pbf);
    // The tile list names no tile: the region is sea to GLO-30, so the layers read no tile.
    fetched(&store, TILE_LIST, "1", Vec::new(), "tileList.txt", b"");

    let region = parse_region(AREA, "name = \"Test\"\nkind = \"geofabrik\"\ncountries = [\"DE\"]\n").unwrap();
    let regions = Regions::new(vec![region]).unwrap();
    let pins = BTreeMap::from([(GLO30.to_string(), "1".to_string()), (TILE_LIST.to_string(), "1".to_string())]);
    let env = Env { name: "test".into(), region: AREA.into(), layers: Vec::new(), pins };
    let steps = Planner.steps(&env, &regions, &store).unwrap();

    let first = plan(&store, &root, &steps).unwrap();
    let mut run = Run::create(&store, "build test").unwrap();
    let context = Context { store: &store, root: &root, sources: &[], http: &Http::new(), limits: Limits::machine() };
    let built = run.build(&context, &steps, &first).unwrap();
    run.finish(None).unwrap();
    let names: Vec<&str> = built.iter().map(|built| built.receipt.step.as_str()).collect();
    assert_eq!(names.len(), 3);
    for name in ["planner/osm", "planner/terrain", "planner/routing"] {
        assert!(names.contains(&name), "{name} is not built");
    }
    let osm = &built.iter().find(|built| built.receipt.step == "planner/osm").unwrap().receipt;
    assert_eq!(osm.files[0].sha256, sha256_hex(&pbf), "the layer is the extract");

    let routing = &built.iter().find(|built| built.receipt.step == "planner/routing").unwrap().receipt;
    let package: BTreeMap<String, PathBuf> = routing
        .files
        .iter()
        .filter_map(|file| Some((file.path.strip_prefix("routing/")?.to_string(), store.object(&file.sha256))))
        .collect();
    let dir = temp.0.join("routing");
    obc_data::engine::view(&package, &dir).unwrap();
    route_engine::open(&dir).unwrap().verify().unwrap();
    let paths: Vec<&str> = routing.files.iter().map(|file| file.path.as_str()).collect();
    for path in ["blocks/blocks.json", "blocks/catalog.json", "routes/9-267-177.json", "routing/route-catalog.json"] {
        assert!(paths.contains(&path), "{path} is not in the layer");
    }

    assert_eq!(plan(&store, &root, &Planner.steps(&env, &regions, &store).unwrap()).unwrap().groups.len(), 0);
    assert!(release(&store, &root, "planner", &steps).unwrap().is_some(), "the store has every layer");
}
