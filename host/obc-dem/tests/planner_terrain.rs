//! The planner terrain step of `obc data` writes the bytes of `planner-dem` from the same GLO-30
//! tile and bounds.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use obc_data::engine::Request;
use obc_dem::step::{planner_terrain, GLO30};

const STEM: &str = "Copernicus_DSM_COG_10_N46_00_E008_00_DEM";

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_step_writes_the_mbtiles_of_planner_dem_from_the_same_tile() {
    let tile = obc_fixtures::file("assistant-terrain", format!("{STEM}.tif"));
    let temp = Temp(std::env::temp_dir().join(format!("obc-dem-planner-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&temp.0);
    let (dem, output) = (temp.0.join("dem"), temp.0.join("layer"));
    std::fs::create_dir_all(&dem).unwrap();
    std::fs::create_dir_all(&output).unwrap();
    std::os::unix::fs::symlink(&tile, dem.join(format!("{STEM}.tif"))).unwrap();

    // Around the Grimsel Pass, inside the one tile.
    let reference = temp.0.join("planner-dem.mbtiles");
    let status = Command::new(env!("CARGO_BIN_EXE_planner-dem"))
        .args(["--dem".as_ref(), dem.as_os_str(), "--bounds".as_ref(), "8.31,46.55,8.34,46.57".as_ref()])
        .args(["--output".as_ref(), reference.as_os_str()])
        .status()
        .unwrap();
    assert!(status.success());

    let files = BTreeMap::from([(format!("{STEM}/{STEM}.tif"), tile)]);
    let request = Request {
        step: "planner/terrain".into(),
        snapshots: BTreeMap::from([(GLO30.to_string(), files)]),
        layers: BTreeMap::new(),
        layer_files: BTreeMap::new(),
        libraries: Vec::new(),
        options: serde_json::json!({"bounds": [8.31, 46.55, 8.34, 46.57]}),
        output: output.clone(),
        metrics: temp.0.join("metrics.json"),
    };
    planner_terrain(&request).unwrap();
    let layer = output.join("terrain.mbtiles");
    assert!(std::fs::read(&layer).unwrap() == std::fs::read(&reference).unwrap(), "the bytes differ");

    let db = rusqlite::Connection::open(&layer).unwrap();
    let tiles: u32 = db.query_row("SELECT count(*) FROM tiles WHERE zoom_level = 12", [], |row| row.get(0)).unwrap();
    assert!(tiles > 0, "the tile gives heights at zoom 12");
}
