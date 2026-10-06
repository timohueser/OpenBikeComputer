//! The terrain layer of `obc data build` over the captured GLO-30 tile of the Grimsel: the same
//! `.obcd` bytes that `obc-bake terrain` writes from the same tile, and they read as the Grimsel.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use obc_bake::terrain::{DemCutter, TerrainBakeOptions, TerrainBakery, TerrainCellStatus, TerrainDoc};
use obc_data::engine::runs::{Context, Limits, Run};
use obc_data::env::Env;
use obc_data::fetch::http::Http;
use obc_data::product::{Product, Unplanned};
use obc_data::regions::{parse_region, Area, Regions};
use obc_data::store::{hash_file, write_atomic, FileRecord, Requested, Snapshot, Store};
use obc_data_steps::maps::{box_poly, Maps, TILE_LIST};
use obc_dem::bake::{V1_CELL_LOG2, V1_POSTING_LOG2};
use obc_dem::step::GLO30;
use obc_elevation::{TerrainReader, TileCache, DEFAULT_TILE_SLOTS};
use obc_formats::io::SliceSource;

const STEM: &str = "Copernicus_DSM_COG_10_N46_00_E008_00_DEM";
const VERSION: &str = "2022-05-09";
/// The Grimsel east of the leaf edge at 8.388608°: one terrain cell, whose source box lies in the
/// one captured tile.
const REGION: &str = "name = \"Grimsel east\"\nkind = \"box\"\nbox = [8.39, 46.48261, 8.46007, 46.66]\n";

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A store whose record of the GLO-30 version has the captured tile, as a fetch of its tile gives it.
fn store_with_tile(dir: &Path, tile: &Path) -> Store {
    let store = Store::at(dir.join("store"));
    let copy = store.partial("tile");
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::copy(tile, &copy).unwrap();
    let (sha256, size) = hash_file(&copy).unwrap();
    store.insert(&copy, &sha256).unwrap();
    let name = format!("{STEM}/{STEM}.tif");
    let url = format!("https://copernicus-dem-30m.s3.amazonaws.com/{name}");
    let retrieved = "2026-09-14T21:39:44Z".into();
    let file = FileRecord { name: name.clone(), url, size, sha256, retrieved };
    store.put_snapshot(&Snapshot { source: GLO30.into(), version: VERSION.into(), files: vec![file] }).unwrap();
    let requested =
        Requested { version: VERSION.into(), params: vec![("tile".into(), STEM.into())], files: vec![name] };
    store.put_requested(GLO30, &requested).unwrap();

    let list = store.partial("list");
    write_atomic(&list, format!("{STEM}\n").as_bytes()).unwrap();
    let (sha256, size) = hash_file(&list).unwrap();
    store.insert(&list, &sha256).unwrap();
    let url = "https://copernicus-dem-30m.s3.amazonaws.com/tileList.txt".into();
    let file = FileRecord { name: "tileList.txt".into(), url, size, sha256, retrieved: String::new() };
    store.put_snapshot(&Snapshot { source: TILE_LIST.into(), version: VERSION.into(), files: vec![file] }).unwrap();
    store
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

/// The path in the layer, or in the tree of `obc-bake terrain` below `cells/terrain/`, to its bytes.
fn layer(store: &Store, root: &Path, regions: &Regions, env: &Env) -> BTreeMap<String, Vec<u8>> {
    without_models(store, env, regions);
    let steps = Maps.steps(env, regions, store).unwrap().steps;
    assert_eq!(steps.len(), 1, "the region is in one leaf");
    let plan = obc_data::engine::plan::plan(store, root, &steps).unwrap();
    let mut run = Run::create(store, "build test").unwrap();
    let context = Context { store, root, sources: &[], http: &Http::new(), copies: None, limits: Limits::machine() };
    let built = run.build(&context, &steps, &plan).unwrap();
    run.finish(None).unwrap();
    let files = built[0].receipt.files.iter();
    files.map(|file| (file.path.clone(), std::fs::read(store.object(&file.sha256)).unwrap())).collect()
}

#[test]
fn a_build_writes_the_terrain_cells_of_obc_bake_terrain_and_they_read_as_the_grimsel() {
    let tile = obc_fixtures::file("assistant-terrain", format!("{STEM}.tif"));
    let temp = Temp(std::env::temp_dir().join(format!("obc-data-terrain-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&temp.0);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let region = parse_region("grimsel-east", REGION).unwrap();
    let Area::Box { bbox } = region.area else { unreachable!() };
    let regions = Regions::new(vec![region]).unwrap();
    let live = BTreeMap::from([
        ((GLO30.into(), Vec::new()), [VERSION.to_string()].into()),
        ((TILE_LIST.into(), Vec::new()), [VERSION.to_string()].into()),
    ]);
    let env = Env { name: "test".into(), region: "grimsel-east".into(), live, ..Env::default() };
    let built = layer(&store_with_tile(&temp.0, &tile), &root, &regions, &env);

    let (sources, extracts, out) = (temp.0.join("sources"), temp.0.join("extracts"), temp.0.join("tree"));
    std::fs::create_dir_all(&sources).unwrap();
    std::os::unix::fs::symlink(&tile, sources.join(format!("{STEM}.tif"))).unwrap();
    std::fs::create_dir_all(&extracts).unwrap();
    std::fs::write(extracts.join("grimsel-east.poly"), box_poly(&bbox)).unwrap();
    let doc = TerrainDoc {
        dataset_id: GLO30.into(),
        dataset_version: "2021-1".into(),
        posting_log2: V1_POSTING_LOG2,
        cell_log2: V1_CELL_LOG2,
        revision: 1,
        attribution: obc_data::sources::attribution(GLO30).into(),
        references: Vec::new(),
    };
    let summary = TerrainBakery {
        regions: &[obc_bake::regions::Region { id: "grimsel-east".into(), name: "Grimsel east".into() }],
        source: &obc_bake::source::LocalExtracts::new(&extracts),
        cutter: &DemCutter::open(&sources, None).unwrap(),
        opts: TerrainBakeOptions { out: out.clone(), doc, force: false, allow_short_reference: false },
    }
    .run(&obc_pack::progress::Progress::silent())
    .unwrap();

    let mut expected = BTreeMap::new();
    for cell in &summary.cells {
        assert_eq!(cell.status, TerrainCellStatus::Baked, "{}", cell.id);
        let path = cell.id.split_once('/').unwrap().1;
        expected.insert(
            format!("terrain/{path}.obcd"),
            std::fs::read(out.join(format!("cells/terrain/{path}.obcd"))).unwrap(),
        );
    }
    expected.insert("metadata/empty.json".into(), b"[]".to_vec());
    assert_eq!(built.keys().collect::<Vec<_>>(), expected.keys().collect::<Vec<_>>());
    assert!(built == expected, "the layer and the tree of obc-bake terrain differ");

    let source = SliceSource(&built["terrain/0600/0528.obcd"]);
    let reader = TerrainReader::parse(&source).unwrap();
    let mut cache = TileCache::<DEFAULT_TILE_SLOTS>::new();
    let furka = reader.sample(&mut cache, 46_572_200, 8_415_300).expect("the Furka Pass has a height");
    assert!((i32::from(furka) - 2429).abs() <= 10, "the Furka Pass reads {furka} m, surveyed 2429 m");
}
