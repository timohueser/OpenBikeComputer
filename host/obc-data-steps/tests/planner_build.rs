//! The planner layers of `obc data build` over a road grid of nine junctions near Freiburg
//! (`data/planner.osm`): the routing package verifies, the overlays derive from it, and a second
//! plan builds nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use obc_data::engine::plan::plan;
use obc_data::engine::release::release;
use obc_data::engine::runs::{Context, Limits, Run as RunLog};
use obc_data::engine::{Built, Run, Step};
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
fn a_build_makes_the_routing_package_and_its_overlays_and_a_second_plan_builds_nothing() {
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
    // The assets and the model read no layer, so this build leaves them out.
    let live = [GLO30, TILE_LIST, "protomaps-assets", "tangrams-icons", "query-model"]
        .map(|id| ((id.to_string(), Vec::new()), BTreeSet::from(["1".to_string()])));
    let env = Env { name: "test".into(), region: AREA.into(), live: BTreeMap::from(live), ..Env::default() };
    let steps = |python: bool| {
        let mut steps = Planner.steps(&env, &regions, &store).unwrap();
        steps.retain(|step| matches!(step.run, Run::Rust(_)) || python && step.name == "planner/overlays");
        steps
    };
    let http = Http::new();
    let context = Context { store: &store, root: &root, sources: &[], http: &http, limits: Limits::machine() };
    let build = |steps: &[Step]| -> Vec<Built> {
        let mut run = RunLog::create(&store, "build test").unwrap();
        let built = run.build(&context, steps, &plan(&store, &root, steps).unwrap()).unwrap();
        run.finish(None).unwrap();
        built
    };

    let built = build(&steps(false));
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

    if Command::new("uv").arg("--version").output().is_err() {
        // CI installs uv, so there a missing uv is a failure, not a skip.
        assert!(std::env::var_os("CI").is_none(), "uv is absent: CI must test the Python step planner/overlays");
        eprintln!("uv is absent: the Python step planner/overlays is not tested");
        return;
    }
    // Machine setup, which a set-up machine has done: the step itself runs offline.
    let sync = Command::new("uv")
        .args(["sync", "--locked", "--inexact", "--group", "planner-maps"])
        .current_dir(&root)
        .status();
    assert!(sync.unwrap().success(), "uv sync of the group planner-maps");
    let built = build(&steps(true));
    let overlays = &built.iter().find(|built| built.receipt.step == "planner/overlays").unwrap().receipt;
    assert_eq!(overlays.command.as_ref().unwrap()[..4], ["uv", "run", "--locked", "--offline"]);
    let archive = std::fs::read(store.object(&overlays.files[0].sha256)).unwrap();
    assert_eq!((&archive[..7], archive[7]), (&b"PMTiles"[..], 3), "overlays.pmtiles is a PMTiles v3 archive");
    assert!(overlays.metrics["tiles"].as_u64().unwrap() > 0, "the cycle route draws tiles");

    assert_eq!(plan(&store, &root, &steps(true)).unwrap().groups.len(), 0);
    let release = release(&store, &root, "planner", AREA, &[], &steps(true)).unwrap();
    assert!(release.is_some(), "the store has every layer");
}
