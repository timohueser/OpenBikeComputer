//! The map cells of `obc data build` over the captured GLO-30 tile of the Grimsel, a synthetic OSM
//! extract (`tests/data/cells.osm`) and synthetic land polygons: each cell has the bytes that one
//! cut of the whole leaf writes, as the planet bake cuts it, and opens in the reader. A copy stands
//! in for the Osmium crop of `maps/osm`. The bands are fine and network, which read the terrain:
//! the semantic levels of coarse and mid raster the whole leaf, which a debug build takes too long
//! for.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use obc_bake::planet::LeafId;
use obc_data::engine::runs::{Context, Limits, Run as RunLog};
use obc_data::engine::{view, Receipt, Request, Run};
use obc_data::env::Env;
use obc_data::fetch::http::Http;
use obc_data::product::Product;
use obc_data::regions::{parse_region, Area, Regions};
use obc_data::store::{hash_file, write_atomic, FileRecord, Requested, Snapshot, Store};
use obc_data_steps::maps::{box_poly, Maps, EXTRACTS, TILE_LIST};
use obc_dem::step::GLO30;
use obc_pack::config::Config;
use obc_pack::cut::{cut, CutOptions};
use obc_pack::grid::{BandTable, CellId};
use obc_pack::progress::Progress;

const STEM: &str = "Copernicus_DSM_COG_10_N46_00_E008_00_DEM";
const PIN: &str = "2022-05-09";
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
    let mut snapshot = store.snapshot(source, PIN).unwrap().unwrap_or_else(|| Snapshot {
        source: source.into(),
        version: PIN.into(),
        files: Vec::new(),
    });
    snapshot.files.push(FileRecord { name: name.into(), url, size, sha256, retrieved: String::new() });
    store.put_snapshot(&snapshot).unwrap();
    if !params.is_empty() {
        let params = params.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect();
        store.put_requested(source, &Requested { version: PIN.into(), params, files: vec![name.into()] }).unwrap();
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
    fetched(&store, "geofabrik-poly", &[("area", AREA)], &format!("{AREA}.poly"), &poly);
    fetched(&store, EXTRACTS, &[("area", AREA)], &format!("{AREA}.osm.pbf"), &pbf);
    let zip = temp.0.join("land.zip");
    land_zip(&zip);
    fetched(&store, "land-polygons", &[], "land-polygons-split-3857.zip", &zip);

    let region = parse_region(AREA, "name = \"Grimsel east\"\nkind = \"geofabrik\"\n").unwrap();
    let regions = Regions::new(vec![region]).unwrap();
    let pins = BTreeMap::from([(GLO30.to_string(), PIN.to_string()), (TILE_LIST.to_string(), PIN.to_string())]);
    let env = Env { name: "test".into(), region: AREA.into(), layers: Vec::new(), pins };
    const BANDS: [&str; 2] = ["fine", "network"];
    let mut steps = Maps.steps(&env, &regions, &store).unwrap();
    steps.retain(|step| !["maps/coarse/", "maps/mid/"].iter().any(|band| step.name.starts_with(band)));
    steps.iter_mut().filter(|step| step.name == "maps/osm").for_each(|step| step.run = Run::Rust(copy));
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
                serde_json::from_slice(&std::fs::read(&layer[&format!("cells/{}/empty.json", cell.band)]).unwrap())
                    .unwrap();
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
}
