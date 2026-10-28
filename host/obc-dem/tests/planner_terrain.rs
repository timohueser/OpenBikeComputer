//! The planner terrain step of `obc data` writes the bytes of `planner-dem` from the same GLO-30
//! tile, OSM and bounds.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use obc_data::engine::Request;
use obc_dem::step::{planner_terrain, GLO30, OSM};

const STEM: &str = "Copernicus_DSM_COG_10_N46_00_E008_00_DEM";
/// Around the Grimsel Pass, inside the one tile.
const BOUNDS: [f64; 4] = [8.31, 46.55, 8.34, 46.57];

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The bytes of the step and of `planner-dem`, from the fixture tile and the OSM file `osm`.
fn bake(temp: &Path, name: &str, osm: Option<&Path>) -> (Vec<u8>, Vec<u8>) {
    let tile = obc_fixtures::file("assistant-terrain", format!("{STEM}.tif"));
    let (dem, output) = (temp.join("dem"), temp.join(name));
    std::fs::create_dir_all(&output).unwrap();
    if !dem.exists() {
        std::fs::create_dir_all(&dem).unwrap();
        std::os::unix::fs::symlink(&tile, dem.join(format!("{STEM}.tif"))).unwrap();
    }
    let text = BOUNDS.map(|value| value.to_string()).join(",");
    let binary = temp.join(format!("{name}.mbtiles"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_planner-dem"));
    command.args(["--dem".as_ref(), dem.as_os_str(), "--bounds".as_ref(), text.as_ref()]);
    command.args(["--output".as_ref(), binary.as_os_str()]);
    if let Some(osm) = osm {
        command.args(["--osm".as_ref(), osm.as_os_str()]);
    }
    assert!(command.status().unwrap().success());

    let files = BTreeMap::from([(format!("{STEM}/{STEM}.tif"), tile)]);
    let extracts = osm.map(|osm| (OSM.to_string(), BTreeMap::from([("area.osm.pbf".to_string(), osm.to_path_buf())])));
    let request = Request {
        step: "planner/terrain".into(),
        snapshots: [(GLO30.to_string(), files)].into_iter().chain(extracts).collect(),
        layers: BTreeMap::new(),
        layer_files: BTreeMap::new(),
        options: serde_json::json!({
            "bounds": BOUNDS,
            "attribution": obc_data::sources::attribution(GLO30),
            "osm_attribution": obc_data::sources::attribution("osm-planet"),
        }),
        output: output.clone(),
        metrics: temp.join("metrics.json"),
        memory_bytes: None,
    };
    planner_terrain(&request).unwrap();
    (std::fs::read(output.join("terrain.mbtiles")).unwrap(), std::fs::read(&binary).unwrap())
}

#[test]
fn the_step_writes_the_mbtiles_of_planner_dem_from_the_same_tile() {
    let temp = Temp(std::env::temp_dir().join(format!("obc-dem-planner-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&temp.0);
    let (step, binary) = bake(&temp.0, "glo30", None);
    assert!(step == binary, "the bytes differ");

    let db = rusqlite::Connection::open(temp.0.join("glo30/terrain.mbtiles")).unwrap();
    let tiles: u32 = db.query_row("SELECT count(*) FROM tiles WHERE zoom_level = 12", [], |row| row.get(0)).unwrap();
    assert!(tiles > 0, "the tile gives heights at zoom 12");

    // An island 200 m across: the water around it is at sea level.
    let osmium = std::env::var_os("OBC_OSMIUM").unwrap_or_else(|| "osmium".into());
    if Command::new(&osmium).arg("--version").output().is_err() {
        assert!(std::env::var_os("CI").is_none(), "CI has Osmium");
        eprintln!("Osmium is unavailable: the OSM half of the test does not run");
        return;
    }
    let ring = [(1, 46.559, 8.324), (2, 46.559, 8.327), (3, 46.561, 8.327), (4, 46.561, 8.324)];
    let nodes: String =
        ring.iter().map(|(id, lat, lon)| format!(r#"<node id="{id}" version="1" lat="{lat}" lon="{lon}"/>"#)).collect();
    let xml = format!(
        r#"<?xml version="1.0"?><osm version="0.6">{nodes}<way id="1" version="1"><nd ref="1"/><nd ref="2"/><nd ref="3"/><nd ref="4"/><nd ref="1"/><tag k="natural" v="coastline"/></way></osm>"#
    );
    std::fs::write(temp.0.join("lake.osm"), xml).unwrap();
    let pbf = temp.0.join("lake.osm.pbf");
    let status = Command::new(&osmium).arg("cat").arg(temp.0.join("lake.osm")).arg("-o").arg(&pbf).status().unwrap();
    assert!(status.success());
    let (shaped, shaped_binary) = bake(&temp.0, "shaped", Some(&pbf));
    assert!(shaped == shaped_binary, "the bytes differ");
    assert!(shaped != step, "the coastline changes the terrain");
}
