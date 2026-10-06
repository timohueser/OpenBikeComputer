//! The planner layers of `obc data build` over a road grid of nine junctions near Freiburg, with a
//! village, two places and an address (`data/planner.osm`): the routing package verifies, the
//! overlays derive from it, the search finds the places and the address, and a second plan builds
//! nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use obc_data::engine::plan::plan;
use obc_data::engine::release::release;
use obc_data::engine::runs::{Context, Limits, Run as RunLog};
use obc_data::engine::{Built, Code, Input, Request, Run, Step};
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

    fetched(&store, "nominatim-country-data", "1", Vec::new(), "nominatim_db-1-py3-none-any.whl", &country_data());

    let text = "name = \"Test\"\nkind = \"geofabrik\"\ncountries = [\"DE\"]\ntime_zone = \"Europe/Berlin\"\n";
    let regions = Regions::new(vec![parse_region(AREA, text).unwrap()]).unwrap();
    let read = [
        GLO30,
        TILE_LIST,
        "protomaps-assets",
        "tangrams-icons",
        "query-model",
        "nominatim-country-data",
        "protomaps-basemaps",
        "natural-earth",
        "water-polygons",
        "land-polygons",
        "daylight-landcover",
        "qrank",
        "pgf-encoding",
    ];
    let live = read.map(|id| ((id.to_string(), Vec::new()), BTreeSet::from(["1".to_string()])));
    let env = Env {
        name: "test".into(),
        region: AREA.into(),
        layers: vec!["sun".into()],
        live: BTreeMap::from(live),
        ..Env::default()
    };
    // Reuse the real places archive for basemap tiles; small local files replace external assets and the model.
    let steps = |python: bool, env: &Env| {
        let mut steps = Planner.steps(&root, env, &regions, &store).unwrap().steps;
        let rust = ["planner/osm", "planner/terrain", "planner/routing"];
        steps.retain(|step| python || rust.contains(&step.name.as_str()));
        for step in &mut steps {
            if step.name == "planner/basemap" {
                step.inputs = vec![Input::layer("planner/places")];
                step.options = serde_json::json!({"kind": "planner/basemap"});
                step.code = Code { paths: Vec::new(), crates: vec!["obc-data".into()], ..Default::default() };
                step.run = Run::Rust(local_files);
            } else if ["planner/assets", "planner/model"].contains(&step.name.as_str()) {
                step.inputs.clear();
                step.options = serde_json::json!({"kind": step.name});
                step.code = Code { paths: Vec::new(), crates: vec!["obc-data-steps".into()], ..Default::default() };
                step.run = Run::Rust(local_files);
            }
        }
        steps
    };
    let http = Http::new();
    let context =
        Context { store: &store, root: &root, sources: &[], http: &http, copies: None, limits: Limits::machine() };
    let build = |steps: &[Step]| -> Vec<Built> {
        let mut run = RunLog::create(&store, "build test").unwrap();
        let built = run.build(&context, steps, &plan(&store, &root, steps).unwrap()).unwrap();
        run.finish(None).unwrap();
        built
    };

    let built = build(&steps(false, &env));
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
        assert!(std::env::var_os("CI").is_none(), "uv is absent: CI must test the Python steps");
        eprintln!("uv is absent: the Python steps are not tested");
        return;
    }
    // Machine setup, which a set-up machine has done: the step itself runs offline.
    let sync = Command::new("uv")
        .args([
            "sync",
            "--locked",
            "--inexact",
            "--group",
            "planner-maps",
            "--group",
            "planner-search",
            "--group",
            "planner-sun",
        ])
        .current_dir(&root)
        .status();
    assert!(sync.unwrap().success(), "uv sync of the planner groups");
    let built = build(&steps(true, &env));
    let metrics = |step: &str| &built.iter().find(|built| built.receipt.step == step).unwrap().receipt.metrics;
    assert_eq!(
        (&metrics("planner/search/records")["pois"], &metrics("planner/search/records")["addresses"]),
        (&3.into(), &4.into())
    );
    let database = |step: &str| {
        let receipt = &built.iter().find(|built| built.receipt.step == step).unwrap().receipt;
        assert!(receipt.metrics["sqlite"].is_string(), "{step} names the SQLite version");
        rusqlite::Connection::open(store.object(&receipt.files[0].sha256)).unwrap()
    };
    let bakery: (String, String) = database("planner/search/pois")
        .query_row("SELECT kind, source FROM places WHERE name = 'Bäckerei'", [], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap();
    assert_eq!(bakery, ("bakery".to_string(), "n10".to_string()));
    let house: (String, String) = database("planner/search/addresses")
        .query_row("SELECT house, source FROM addresses", [], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap();
    assert_eq!(house, ("3".to_string(), "n13".to_string()));
    // The bakery and the drinking water; the village is no rider place.
    assert_eq!(metrics("planner/places")["places"], 2);
    let overlays = &built.iter().find(|built| built.receipt.step == "planner/overlays").unwrap().receipt;
    assert_eq!(
        overlays.command.as_ref().unwrap()[..6],
        ["env", "PYTHONHASHSEED=0", "uv", "run", "--locked", "--offline"]
    );
    let archive = std::fs::read(store.object(&overlays.files[0].sha256)).unwrap();
    assert_eq!((&archive[..7], archive[7]), (&b"PMTiles"[..], 3), "overlays.pmtiles is a PMTiles v3 archive");
    assert!(overlays.metrics["tiles"].as_u64().unwrap() > 0, "the cycle route draws tiles");
    for kind in ["places", "overlays", "terrain", "sun"] {
        let name = format!("planner/{kind}/grid");
        let receipt = &built.iter().find(|built| built.receipt.step == name).unwrap().receipt;
        let index = receipt.files.iter().find(|file| file.path == "index.json").unwrap();
        let index: serde_json::Value =
            serde_json::from_slice(&std::fs::read(store.object(&index.sha256)).unwrap()).unwrap();
        assert_eq!(index["kind"], kind);
        assert!(index["files"][format!("maps/{kind}.json")].is_object());
        for entry in index["files"].as_object().unwrap().values() {
            let hash = entry["transport"]["sha256"].as_str().unwrap();
            assert!(receipt.files.iter().any(|file| file.path == format!("objects/{hash}")));
        }
    }

    let verification = temp.0.join("verified");
    for receipt in built
        .iter()
        .map(|built| &built.receipt)
        .filter(|receipt| receipt.step.ends_with("/grid") || receipt.step == "planner/index")
    {
        for file in &receipt.files {
            if receipt.step.ends_with("/grid") && file.path == "index.json" {
                let path = verification.join("indexes").join(&receipt.step).join("index.json");
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::hard_link(store.object(&file.sha256), path).unwrap();
            }
            if file.path.starts_with("objects/") || receipt.step == "planner/index" && file.path == "release.json" {
                let path = verification.join(&file.path);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                if !path.exists() {
                    std::fs::hard_link(store.object(&file.sha256), path).unwrap();
                }
            }
        }
    }
    let check = Command::new("uv")
        .args([
            "run",
            "--locked",
            "--offline",
            "python",
            "-c",
            r#"
import copy, json, pathlib, sqlite3, sys
from tools import planner_runtime as runtime, planner_offline as offline, planner_downloads as downloads, planner_grid_index as grid_index
source = pathlib.Path(sys.argv[1])
_, document = runtime.release(source, include_sources=False)
assert document['osm_sha256'] == sys.argv[2]
indexes = {index['kind']: index for path in (source / 'indexes').rglob('index.json')
    for index in [json.loads(path.read_bytes())]}
assert indexes['sun']['metadata']['sun_format'] == 3
assert indexes['sun']['metadata']['terrain_sha256'] == indexes['terrain']['source']['sha256']
assert set(indexes['sun']['files']) == {'maps/sun.json'}
options = {key: document[key] for key in ('region', 'bounds', 'attribution', 'landcover_attribution')}
for kind, field, value in [('places', 'osm_sha256', '0' * 64), ('addresses', 'bounds', [0, 0, 1, 1]),
                          ('sun', 'terrain_sha256', '0' * 64)]:
    wrong = copy.deepcopy(indexes)
    wrong[kind]['metadata'][field] = value
    try:
        grid_index.compose(wrong, options)
    except ValueError:
        pass
    else:
        raise AssertionError('an inconsistent source or coverage is rejected')
offline.materialize(source, source / 'runtime', document, ('offline/', 'routing/', 'search/'))
catalog = json.loads((source / 'runtime/offline/catalog.json').read_bytes())
assert not any(block['kind'] in ('terrain', 'sun') for block in catalog['map_blocks'])
assert catalog['release']['terrain_bounds'] == document['terrain_bounds']
assert catalog['release']['osm_sha256'] == document['osm_sha256']
search = json.loads((source / 'runtime/search/test.grid.json').read_bytes())
for cell in search['cells']:
    for name in cell['files']:
        with sqlite3.connect(f'{(source / "runtime/search" / name).as_uri()}?mode=ro', uri=True) as db:
            assert db.execute('PRAGMA quick_check').fetchone() == ('ok',)
            metadata = {key: json.loads(value) for key, value in db.execute('SELECT key,value FROM metadata')}
            assert metadata['osm_sha256'] == document['osm_sha256'] and metadata['bounds'] == cell['bounds']
            if metadata['component'] == 'pois':
                assert db.execute("SELECT source FROM places WHERE name='Bäckerei'").fetchone() == ('n10',)
            else:
                assert db.execute('SELECT house,source FROM addresses').fetchone() == ('3', 'n13')
service = downloads.Downloads(source / 'runtime/offline', source / 'selections', 1000000, 'https://example.org/objects')
selection = service.prepare({'bounds': document['bounds']})
bundle = json.loads((source / 'selections' / selection['id'] / 'bundle.json').read_bytes())
assert any(name.startswith('search/tiles/pois/') for name in bundle['files'])
assert any(name.startswith('search/tiles/addresses/') for name in bundle['files'])
assert any(name.startswith('routing/packs/') for name in bundle['files'])
for name, entry in catalog['files'].items():
    offline.verify(source / 'objects' / entry['transport']['sha256'], entry['transport'])
"#,
        ])
        .arg(&verification)
        .arg(sha256_hex(&pbf))
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(check.success(), "real grid objects and empty terrain form a verified offline selection");

    let mut climate = env.clone();
    climate.layers.push("climate".into());
    climate.moves.insert("era5-land".into(), Some(DAY.into()));
    let climate = plan(&store, &root, &steps(true, &climate)).unwrap();
    assert_eq!(
        climate.builds().map(|build| build.step.as_str()).collect::<Vec<_>>(),
        ["planner/climate", "planner/climate/grid", "planner/index"],
        "optional climate reuses every other layer"
    );

    assert_eq!(plan(&store, &root, &steps(true, &env)).unwrap().groups.len(), 0);
    let release = release(&store, &root, "planner", AREA, &[], &steps(true, &env)).unwrap();
    assert!(release.is_some(), "the store has every layer");
    let release = release.unwrap();
    for layer in release.layers.iter().filter(|layer| layer.step.ends_with("/grid")) {
        assert!(layer.files.iter().any(|file| file.path == "index.json"));
        assert!(layer.client_files().all(|file| file.path.starts_with("objects/")));
    }
}

fn local_files(request: &Request) -> Result<(), String> {
    if request.options["kind"] == "planner/basemap" {
        let source = &request.layers["planner/places"]["places.pmtiles"];
        return std::fs::hard_link(source, request.output.join("basemap.pmtiles")).map_err(|e| e.to_string());
    }
    let files: &[(&str, &[u8])] = match request.options["kind"].as_str() {
        Some("planner/assets") => &[
            ("assets/fonts/Noto Sans Regular/0-255.pbf", b"fixture glyph range"),
            ("assets/fonts/OFL.txt", b"fixture font credit"),
            ("assets/sprites/v4.json", b"{}"),
            ("assets/sprites/LICENSE.txt", b"fixture sprite credit"),
        ],
        Some("planner/model") => &[("model/model.int8.onnx", b"fixture model"), ("model/labels.json", b"[]")],
        _ => return Err("unknown fixture producer".into()),
    };
    for (name, bytes) in files {
        let path = request.output.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(path, bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// A stand-in for the Nominatim archive: the settings of Germany, and a country grid in which the
/// square from 7° to 9° east and 47° to 49° north is Germany.
fn country_data() -> Vec<u8> {
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let mut add = |name: &str, bytes: &[u8]| {
        archive.start_file(format!("nominatim_db/resources/{name}"), zip::write::SimpleFileOptions::default()).unwrap();
        archive.write_all(bytes).unwrap();
    };
    let settings = "de:\n  names:\n    name:\n      default: Deutschland\n  postcode:\n    pattern: ddddd\n";
    add("settings/country_settings.yaml", settings.as_bytes());
    let levels = r#"[{"tags": {"place": {"village": [19, 16]}, "highway": {"": 26}}}]"#;
    add("settings/address-levels.json", levels.as_bytes());
    // A row is the country, the area and the polygon as hexadecimal little-endian WKB.
    let mut wkb = vec![1, 3, 0, 0, 0, 1, 0, 0, 0, 5, 0, 0, 0];
    for (x, y) in [(7.0, 47.0), (9.0, 47.0), (9.0, 49.0), (7.0, 49.0), (7.0, 47.0)] {
        wkb.extend([f64::to_le_bytes(x), f64::to_le_bytes(y)].concat());
    }
    let hex: String = wkb.iter().map(|byte| format!("{byte:02x}")).collect();
    let mut grid = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    grid.write_all(format!("de\t4\t{hex}\n").as_bytes()).unwrap();
    add("country_osm_grid.sql.gz", &grid.finish().unwrap());
    archive.finish().unwrap().into_inner()
}
