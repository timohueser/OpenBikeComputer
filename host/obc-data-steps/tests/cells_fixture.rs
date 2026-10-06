//! The map cells of `obc data build` over the captured GLO-30 tile of the Grimsel, a synthetic OSM
//! extract (`tests/data/cells.osm`) and synthetic land polygons: each cell has the bytes that one
//! cut of the whole leaf writes, as the planet bake cuts it, and opens in the reader. A copy stands
//! in for the Osmium crop of `maps/osm`. The bands are fine and network, which read the terrain:
//! authored coarse and mid geometry uses the production serializer to avoid a whole-leaf
//! semantic raster in debug. A compiled landmark and a compiled peak on the summit of the extract stand in for the
//! compile of a capture, and give one artifact each.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use obc_bake::planet::LeafId;
use obc_data::engine::runs::{Context, Limits, Run as RunLog};
use obc_data::engine::{view, Receipt, Request, Run};
use obc_data::env::Env;
use obc_data::fetch::http::Http;
use obc_data::product::{Product, Unplanned};
use obc_data::regions::{parse_region, Area, Regions};
use obc_data::store::{hash_file, write_atomic, FileRecord, Requested, Snapshot, Store};
use obc_data_steps::maps::{box_poly, Maps, EXTRACTS, TILE_LIST};
use obc_dem::step::GLO30;
use obc_formats::obcm::landmarks::{LandmarkRecord, RECORD_LEN, SECTION_HEADER_LEN};
use obc_formats::obcm::SourceId;
use obc_pack::config::Config;
use obc_pack::cut::{cut, CutOptions};
use obc_pack::grid::{BandTable, CellId};
use obc_pack::progress::Progress;

const STEM: &str = "Copernicus_DSM_COG_10_N46_00_E008_00_DEM";
const VERSION: &str = "2022-05-09";
const AREA: &str = "europe/grimsel-east";
/// The Grimsel east of the leaf edge at 8.388608°: one terrain cell, whose source box lies in the
/// one captured tile.
const REGION: &str = "name = \"Grimsel east\"\nkind = \"box\"\nbox = [8.39, 46.48261, 8.46007, 46.66]\n";

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Record the file at `path` as `name` of `source@version`, as a fetch with `params` gives it.
fn fetched(store: &Store, source: &str, params: &[(&str, &str)], name: &str, path: &Path) {
    let copy = store.partial("file");
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::copy(path, &copy).unwrap();
    let (sha256, size) = hash_file(&copy).unwrap();
    store.insert(&copy, &sha256).unwrap();
    let url = format!("https://example.org/{name}");
    let mut snapshot = store.snapshot(source, VERSION).unwrap().unwrap_or_else(|| Snapshot {
        source: source.into(),
        version: VERSION.into(),
        files: Vec::new(),
    });
    snapshot.files.push(FileRecord { name: name.into(), url, size, sha256, retrieved: String::new() });
    store.put_snapshot(&snapshot).unwrap();
    if !params.is_empty() {
        let params = params.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect();
        store.put_requested(source, &Requested { version: VERSION.into(), params, files: vec![name.into()] }).unwrap();
    }
}

/// Record a fetch without files of each national model that the step list asks for: the store has
/// no national data of the Grimsel, so the terrain reads GLO-30 alone.
fn without_models(store: &Store, env: &Env, regions: &Regions) {
    let Err(Unplanned::NeedsFetch(wanted)) = Maps.steps(env, regions, store) else { return };
    for fetch in wanted.iter().filter(|fetch| fetch.source.starts_with("dtm-")) {
        let requested = Requested { version: VERSION.into(), params: fetch.params.clone(), files: Vec::new() };
        store.put_requested(&fetch.source, &requested).unwrap();
    }
}

/// A zip of `land-polygons-split-3857` whose one polygon is land over the whole of N46 E008.
fn land_zip(path: &Path) {
    let merc = |lon: f64, lat: f64| {
        let r = 6_378_137.0;
        (r * lon.to_radians(), r * (std::f64::consts::FRAC_PI_4 + lat.to_radians() / 2.0).tan().ln())
    };
    let ((x0, y0), (x1, y1)) = (merc(8.0, 46.0), merc(9.0, 47.0));
    let (mut header, mut record) = (vec![0u8; 100], Vec::new());
    header[0..4].copy_from_slice(&9994i32.to_be_bytes());
    header[28..32].copy_from_slice(&1000i32.to_le_bytes());
    header[32..36].copy_from_slice(&5i32.to_le_bytes());
    record.extend(5i32.to_le_bytes());
    [x0, y0, x1, y1].iter().for_each(|v| record.extend(v.to_le_bytes()));
    record.extend(1i32.to_le_bytes());
    record.extend(5i32.to_le_bytes());
    record.extend(0i32.to_le_bytes());
    // An outer ring is clockwise.
    for (x, y) in [(x0, y0), (x0, y1), (x1, y1), (x1, y0), (x0, y0)] {
        record.extend(x.to_le_bytes());
        record.extend(y.to_le_bytes());
    }
    let mut shp = header;
    shp.extend(1i32.to_be_bytes());
    shp.extend((record.len() as i32 / 2).to_be_bytes());
    shp.extend(record);
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let options: zip::write::FileOptions<'_, ()> = Default::default();
    zip.start_file("land-polygons-split-3857/land_polygons.shp", options).unwrap();
    zip.write_all(&shp).unwrap();
    zip.finish().unwrap();
}

/// The OSM of each leaf: the extract as it is.
fn copy(request: &Request) -> Result<(), String> {
    let extract = request.snapshots[EXTRACTS].values().next().ok_or("no extract")?;
    std::fs::create_dir(request.output.join("osm")).map_err(|e| e.to_string())?;
    for leaf in request.options["leaves"].as_array().ok_or("no leaves")? {
        let leaf = LeafId { i: leaf[0].as_i64().unwrap(), j: leaf[1].as_i64().unwrap() };
        std::fs::copy(extract, request.output.join(obc_bake::step::leaf_pbf(leaf))).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[test]
fn a_build_writes_the_cells_of_one_cut_of_the_leaf_and_they_open_in_the_reader() {
    let temp = Temp(std::env::temp_dir().join(format!("obc-data-cells-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&temp.0);
    std::fs::create_dir_all(&temp.0).unwrap();
    // The cutter unpacks the land polygons below this directory.
    std::env::set_var("OBCM_CACHE_DIR", temp.0.join("cache"));
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let pbf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/cells.osm.pbf");

    let store = Store::at(temp.0.join("store"));
    let tile = obc_fixtures::file("assistant-terrain", format!("{STEM}.tif"));
    fetched(&store, GLO30, &[("tile", STEM)], &format!("{STEM}/{STEM}.tif"), &tile);
    let file = |name: &str, bytes: &[u8]| {
        let path = temp.0.join(name);
        write_atomic(&path, bytes).unwrap();
        path
    };
    fetched(&store, TILE_LIST, &[], "tileList.txt", &file("tileList.txt", format!("{STEM}\n").as_bytes()));
    let Area::Box { bbox } = parse_region("grimsel-east", REGION).unwrap().area else { unreachable!() };
    let poly = file("area.poly", box_poly(&bbox).as_bytes());
    let index = serde_json::json!({"type":"FeatureCollection", "features":[{"type":"Feature",
        "properties":{"id":"grimsel-east","name":"Grimsel east","parent":null,"urls":{"pbf":format!("https://download.geofabrik.de/{AREA}-latest.osm.pbf")}},
        "geometry":{"type":"Polygon","coordinates":[[[bbox.west,bbox.south],[bbox.east,bbox.south],[bbox.east,bbox.north],[bbox.west,bbox.north],[bbox.west,bbox.south]]]}}]});
    fetched(&store, "geofabrik-index", &[], "index-v1.json", &file("index.json", &serde_json::to_vec(&index).unwrap()));
    fetched(&store, "geofabrik-poly", &[("area", AREA)], &format!("{AREA}.poly"), &poly);
    fetched(&store, EXTRACTS, &[("area", AREA)], &format!("{AREA}.osm.pbf"), &pbf);
    let zip = temp.0.join("land.zip");
    land_zip(&zip);
    fetched(&store, "land-polygons", &[], "land-polygons-split-3857.zip", &zip);
    // The Wikimedia captures, which only the compiles read, and this test has none.
    let [osm, poly] = [&pbf, &poly].map(|path| format!("sha256:{}", hash_file(path).unwrap().0));
    let code = obc_pack::step::capture_code();
    for collection in ["landmarks", "peaks"] {
        let params = [
            ("collection", collection),
            ("area", AREA),
            ("osm", osm.as_str()),
            ("poly", poly.as_str()),
            ("code", code.as_str()),
        ];
        for source in obc_pack::step::CAPTURES {
            fetched(&store, source, &params, &format!("#{collection}=0/recipe.json"), &zip);
        }
    }

    let region = parse_region(AREA, "name = \"Grimsel east\"\nkind = \"geofabrik\"\n").unwrap();
    let regions = Regions::new(vec![region]).unwrap();
    let live = BTreeMap::from([
        ((GLO30.into(), Vec::new()), [VERSION.to_string()].into()),
        ((TILE_LIST.into(), Vec::new()), [VERSION.to_string()].into()),
    ]);
    let env = Env { name: "test".into(), region: AREA.into(), live, ..Env::default() };
    const BANDS: [&str; 2] = ["fine", "network"];
    without_models(&store, &env, &regions);
    let listed = Maps.steps(&env, &regions, &store).unwrap();
    assert!(listed.blocked.is_empty(), "{:?}", listed.blocked);
    let mut steps = listed.steps;

    for step in &mut steps {
        match step.name.as_str() {
            "maps/osm" => step.run = Run::Rust(copy),
            "maps/landmark-content" => step.run = Run::Rust(landmark_content),
            "maps/peak-content" => step.run = Run::Rust(peak_content),
            name if name.starts_with("maps/coarse/") || name.starts_with("maps/mid/") => {
                step.run = Run::Rust(authored_geometry)
            }
            _ => {}
        }
    }
    let plan = obc_data::engine::plan::plan(&store, &root, &steps).unwrap();
    let mut run = RunLog::create(&store, "build test").unwrap();
    let context = Context { store: &store, root: &root, sources: &[], http: &Http::new(), limits: Limits::machine() };
    let built = run.build(&context, &steps, &plan).unwrap();
    run.finish(None).unwrap();
    let layers: BTreeMap<&str, &Receipt> =
        built.iter().map(|built| (built.receipt.step.as_str(), &built.receipt)).collect();
    let objects = |layer: &str| -> BTreeMap<String, PathBuf> {
        layers[layer].files.iter().map(|file| (file.path.clone(), store.object(&file.sha256))).collect()
    };

    let release = obc_data::engine::release::release(&store, &root, "maps", AREA, &[], &steps).unwrap().unwrap();
    Maps.verify(None, &release, &store).unwrap();
    let pointer = Maps.pointer().unwrap()(&release, &store).unwrap();
    assert!(pointer.named.contains_key("schema.json") && pointer.named.contains_key("LICENSE.txt"));
    let catalog: obc_pack::catalog::Catalog = serde_json::from_value(pointer.document.into()).unwrap();
    assert_eq!(catalog.schema.sha256, obc_data::store::sha256_hex(&pointer.named["schema.json"]));
    assert_eq!(catalog.regions.len(), 1);
    assert!(catalog.regions[0].article_bytes.unwrap() > 0);
    assert!(release
        .layers
        .iter()
        .filter(|layer| layer.step.starts_with("maps/terrain/")
            || ["coarse", "mid", "fine", "network"].iter().any(|band| layer.step.starts_with(&format!("maps/{band}/"))))
        .all(|layer| layer.client_files().all(|file| !file.path.starts_with("metadata/"))));

    // One cut of every band over the leaf, as the planet bake cuts a leaf.
    let leaf = CellId::new(23, 37, 33).unwrap();
    let terrain = temp.0.join("terrain");
    view(&objects("maps/terrain/0037-0033"), &terrain).unwrap();
    let options =
        |band: &str| steps.iter().find(|step| step.name == format!("maps/{band}/0037-0033")).unwrap().options.clone();
    let bands = BandTable::recommended();
    let mut select = Vec::new();
    for band in bands.bands.iter().filter(|band| BANDS.contains(&band.id.as_str())) {
        let cells = options(&band.id)["cells"].as_array().unwrap().clone();
        let cell = |cell: &serde_json::Value| {
            CellId::new(band.cell_log2, cell[0].as_i64().unwrap(), cell[1].as_i64().unwrap())
        };
        select.extend(cells.iter().map(|value| cell(value).unwrap()));
    }
    let config = Config::parse(include_str!("../../../builder/presets/schema.json")).unwrap();
    let opts = CutOptions {
        bands,
        select,
        only_bands: BANDS.map(String::from).to_vec(),
        land: Some(zip),
        terrain: Some(terrain),
        source_extent: Some(leaf.square()),
        ..CutOptions::default()
    };
    let out = temp.0.join("cut");
    let summary = cut(&[pbf.to_string_lossy().into_owned()], &config, &out, &opts, &Progress::silent()).unwrap();
    assert!(summary.cells.iter().any(|cell| cell.band == "network" && cell.nav_edges > 0 && cell.pois > 0));
    assert!(summary.cells.iter().any(|cell| cell.band == "fine" && !cell.empty));

    let mut expected: BTreeMap<String, BTreeSet<String>> =
        BANDS.iter().map(|band| (band.to_string(), BTreeSet::new())).collect();
    for cell in &summary.cells {
        let layer = objects(&format!("maps/{}/0037-0033", cell.band));
        if cell.empty {
            let empty: Vec<String> =
                serde_json::from_slice(&std::fs::read(&layer["metadata/empty.json"]).unwrap()).unwrap();
            assert!(empty.contains(&cell.id.to_string()), "{} {}", cell.band, cell.id);
            continue;
        }
        let object = &layer[&cell.path];
        assert!(
            std::fs::read(object).unwrap() == std::fs::read(out.join(&cell.path)).unwrap(),
            "{} differs",
            cell.path
        );
        obc_bake::verify::verify_cell(object, cell.id.square()).unwrap();
        expected.get_mut(&cell.band).unwrap().insert(cell.path.clone());
    }
    for (band, paths) in expected {
        let layer = objects(&format!("maps/{band}/0037-0033"));
        let cells: BTreeSet<String> = layer.into_keys().filter(|path| path.ends_with(".obcm")).collect();
        assert_eq!(cells, paths, "the layer of {band} has other cells");
    }

    // The summit's network cell owns the landmark, linked to the summit node, and the peak.
    let owner = CellId::containing(18, 46_610_000, 8_440_000);
    let artifact = |layer: &str, class: &str| {
        let path = format!("{class}/{:04}/{:04}.bin", owner.i, owner.j);
        assert_eq!(objects(layer).into_keys().collect::<Vec<_>>(), std::slice::from_ref(&path), "{layer}");
        std::fs::read(&objects(layer)[&path]).unwrap()
    };
    let landmarks = artifact("maps/landmarks/0037-0033", "landmarks");
    assert_eq!(u32::from_le_bytes(landmarks[..4].try_into().unwrap()), 1);
    let record = &landmarks[SECTION_HEADER_LEN..SECTION_HEADER_LEN + RECORD_LEN];
    let record = LandmarkRecord::decode(record.try_into().unwrap()).unwrap();
    assert_eq!((record.qid, record.osm.map(|osm| osm.source)), (1, Some(SourceId::osm(1, 20))));
    let peaks = artifact("maps/peaks/0037-0033", "peaks");
    assert_eq!(u32::from_le_bytes(peaks[4..8].try_into().unwrap()), 1, "one association");
    // Reuse is selected from receipts. An unchanged selection reads no binary payload again.
    let network = layers["maps/network/0037-0033"].files.iter().find(|file| file.path.ends_with(".obcm")).unwrap();
    let path = store.object(&network.sha256);
    let backup = temp.0.join("retained-network.obcm");
    std::fs::rename(&path, &backup).unwrap();
    Maps.verify(Some(&release), &release, &store).unwrap();
    assert!(Maps.verify(None, &release, &store).unwrap_err().contains("artifact"));
    std::fs::rename(backup, path).unwrap();
    for step in &mut steps {
        if step.name == "maps/network/0037-0033" {
            step.options["malformed_fixture"] = true.into();
            step.run = Run::Rust(malformed_network);
        }
    }
    let changed = obc_data::engine::plan::plan(&store, &root, &steps).unwrap();
    let mut run = RunLog::create(&store, "build changed fixture").unwrap();
    run.build(&context, &steps, &changed).unwrap();
    run.finish(None).unwrap();
    let next = obc_data::engine::release::release(&store, &root, "maps", AREA, &[], &steps).unwrap().unwrap();
    let error = Maps.verify(Some(&release), &next, &store).unwrap_err();
    assert!(error.contains("not a readable OBCM"), "{error}");
}

/// The credit of an article of the stand-in compiles.
fn credit() -> serde_json::Value {
    serde_json::json!({"source_url": "https://en.wikipedia.org/w/index.php?title=Testhorn&oldid=1", "revision": "1",
        "license_url": "https://creativecommons.org/licenses/by-sa/4.0/", "original_notices": "Authors"})
}

fn write_content(request: &Request, path: &str, content: serde_json::Value) -> Result<(), String> {
    let path = request.output.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(path, serde_json::to_vec(&content).unwrap()).map_err(|e| e.to_string())
}

/// The compiled landmarks: the summit as a landmark.
fn landmark_content(request: &Request) -> Result<(), String> {
    let counts = serde_json::to_value(obc_pack::landmarks::Counts::default()).unwrap();
    let content = serde_json::json!({"schema": 2, "input_sha256": "input", "policy_sha256": "policy",
        "category_policy_sha256": "categories", "languages": ["en", "de", "fr", "es"], "source_coverage": {},
        "counts": counts, "candidate_qids": [], "omissions": [],
        "records": [{"qid": "Q1", "name": "Testhorn", "category": 1, "latitude": 46.61, "longitude": 8.44,
            "default_language": "en", "fallback_sources": [], "photo": null,
            "variants": [{"language": "en", "text_pages": ["A summit."], "attribution": credit()}]}]});
    write_content(request, "landmarks/content.json", content)
}

/// The compiled peaks: an article of the summit node.
fn peak_content(request: &Request) -> Result<(), String> {
    let counts = serde_json::to_value(obc_pack::landmarks::Counts::default()).unwrap();
    let content = serde_json::json!({"schema": 1, "collection": "peaks", "input_sha256": "input",
        "policy_sha256": "policy", "languages": ["en", "de", "fr", "es"], "source_coverage": {}, "counts": counts,
        "omissions": [],
        "records": [{"id": "Q1", "name": "Testhorn", "default_language": "en", "fallback_sources": [], "photo": null,
            "variants": [{"language": "en", "text_pages": ["A summit."], "attribution": credit()}]}],
        "associations": [{"node_id": 20, "article_id": "Q1", "latitude": 46.61, "longitude": 8.44}]});
    write_content(request, "peaks/peaks.json", content)
}

/// Authored coarse/mid geometry, serialized with the same style/profile tables as the real cuts.
fn authored_geometry(request: &Request) -> Result<(), String> {
    use obc_pack::serialize::{serialize_lods, Feature, Kind, LodLayer, Node};
    let config = Config::parse(include_str!("../../../builder/presets/schema.json"))?;
    let band = request.options["band"].as_str().ok_or("missing band")?;
    let table = BandTable::recommended();
    let band = table.bands.iter().find(|definition| definition.id == band).ok_or("missing band definition")?;
    for pair in request.options["cells"].as_array().ok_or("missing cells")? {
        let id = CellId::new(
            band.cell_log2,
            pair[0].as_i64().ok_or("invalid row")?,
            pair[1].as_i64().ok_or("invalid column")?,
        )?;
        let square = id.square();
        let feature = Feature {
            style_id: config.styles()[0].id,
            kind: Kind::Line,
            rings: vec![vec![
                ((square.0 + 100) as f64 / 1e6, (square.1 + 100) as f64 / 1e6),
                ((square.2 - 100) as f64 / 1e6, (square.3 - 100) as f64 / 1e6),
            ]],
        };
        let lods: Vec<_> = config
            .lods
            .iter()
            .enumerate()
            .map(|(level, lod)| LodLayer {
                max_mpp: lod.max_mpp,
                chunk_size: config.chunk_size,
                root: Node::Leaf {
                    bbox: square,
                    features: if band.lods.contains(&level) { vec![feature.clone()] } else { Vec::new() },
                },
            })
            .collect();
        let (body, dropped) = serialize_lods(
            &lods,
            &config.styles(),
            config.marker_color,
            square,
            &[],
            &Default::default(),
            &config.routing.profiles,
            &mut obc_elevation::NullElevation,
        );
        assert_eq!(dropped, 0);
        let path = request.output.join(format!("cells/{}/{:04}/{:04}.obcm", band.id, id.i, id.j));
        write_atomic(&path, &body)?;
    }
    write_atomic(&request.output.join("metadata/empty.json"), b"[]")
}

/// A structurally invalid payload whose valid header lets metadata generation complete.
fn malformed_network(request: &Request) -> Result<(), String> {
    obc_pack::step::cells(request)?;
    let width = obc_pack::grid::id_width(18);
    let pair = &request.options["cells"][0];
    let path = request.output.join(format!(
        "cells/network/{:0width$}/{:0width$}.obcm",
        pair[0].as_i64().ok_or("row")?,
        pair[1].as_i64().ok_or("column")?
    ));
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?
        .set_len(obc_formats::obcm::HEADER_LEN as u64)
        .map_err(|e| e.to_string())
}
