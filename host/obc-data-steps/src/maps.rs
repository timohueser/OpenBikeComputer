//! The device maps. Each layer covers one leaf of the `2^23` grid that the region touches:
//! `maps/terrain/<i>-<j>` holds the terrain cells of the region in leaf `(i, j)`.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use obc_bake::coverage::Coverage;
use obc_data::engine::{Code, Input, Run, Step};
use obc_data::env::Env;
use obc_data::product::{read, Product, Unplanned, Wanted};
use obc_data::regions::{Area, Bbox, Regions};
use obc_data::store::Store;
use obc_dem::bake::{V1_CELL_LOG2, V1_POSTING_LOG2};
use obc_dem::step::GLO30;
use obc_pack::grid::{id_width, CellId};

/// The cell of the planet bake, and of every device-map layer.
const LEAF_LOG2: u32 = obc_bake::planet::SOURCE_LEAF_LOG2;
const POLY: &str = "geofabrik-poly";
/// The names of the GLO-30 tiles: a square that it does not name is sea.
pub const TILE_LIST: &str = "copernicus-glo-30-tiles";

pub struct Maps;

impl Product for Maps {
    fn name(&self) -> &'static str {
        "maps"
    }

    fn steps(&self, env: &Env, regions: &Regions, store: &Store) -> Result<Vec<Step>, Unplanned> {
        let mut wanted = Vec::new();
        let outlines = outlines(env, regions, store, &mut wanted)?;
        let tile_list = text(env, store, TILE_LIST, &[], &mut wanted)?;
        let (Some(outlines), Some(tile_list)) = (outlines, tile_list) else {
            return Err(Unplanned::NeedsFetch(wanted));
        };
        let land: HashSet<&str> = tile_list.lines().map(str::trim).collect();
        let glo30 =
            env.version(GLO30).ok_or_else(|| invalid(format!("data/env/{}.toml pins no `{GLO30}`", env.name)))?;
        let cells: BTreeSet<CellId> = outlines.iter().flat_map(|outline| outline.cells(V1_CELL_LOG2.into())).collect();
        let mut leaves: BTreeMap<(i64, i64), Vec<CellId>> = BTreeMap::new();
        let shift = LEAF_LOG2 - u32::from(V1_CELL_LOG2);
        for cell in cells {
            leaves.entry((cell.i >> shift, cell.j >> shift)).or_default().push(cell);
        }
        Ok(leaves.into_iter().map(|(leaf, cells)| terrain(leaf, &cells, &land, glo30)).collect())
    }
}

/// The terrain cells of one leaf, from the GLO-30 tiles that the square of each cell reaches. A
/// square that the tile list does not name is sea, and has no tile to read.
fn terrain((i, j): (i64, i64), cells: &[CellId], land: &HashSet<&str>, glo30: &str) -> Step {
    let source_box = |cell: &CellId| obc_bake::terrain::source_bbox([*cell]).expect("a cell has a box");
    let tiles = cells.iter().flat_map(|cell| obc_dem::fetch::tiles_for(source_box(cell)));
    let tiles: BTreeSet<String> = tiles.map(|tile| tile.stem()).filter(|tile| land.contains(tile.as_str())).collect();
    let params: Vec<_> = tiles.into_iter().map(|tile| ("tile".to_string(), tile)).collect();
    // A leaf at sea reads no snapshot: an input without params reads every file of its version.
    let inputs = match params.is_empty() {
        true => Vec::new(),
        false => vec![Input::Snapshot { source: GLO30.into(), version: glo30.into(), params, files: Vec::new() }],
    };
    let width = id_width(LEAF_LOG2);
    Step {
        name: format!("maps/terrain/{i:0width$}-{j:0width$}"),
        inputs,
        options: serde_json::json!({
            "posting_log2": V1_POSTING_LOG2,
            "cell_log2": V1_CELL_LOG2,
            "cells": cells.iter().map(|cell| [cell.i, cell.j]).collect::<Vec<_>>(),
        }),
        code: Code { paths: Vec::new(), crates: vec!["obc-dem".into()] },
        outputs: vec!["terrain".into()],
        run: Run::Rust(obc_dem::step::terrain),
    }
}

/// The outline of each region that `env.region` resolves to, or `None` while the store lacks the
/// `.poly` of a Geofabrik area; then `wanted` has its fetch.
fn outlines(
    env: &Env,
    regions: &Regions,
    store: &Store,
    wanted: &mut Vec<Wanted>,
) -> Result<Option<Vec<Coverage>>, Unplanned> {
    let mut outlines = Some(Vec::new());
    for id in regions.leaves(&env.region).map_err(Unplanned::Invalid)? {
        let poly = match &regions.get(id).expect("a leaf region exists").area {
            Area::Box { bbox } => Some(box_poly(bbox)),
            Area::Geofabrik => text(env, store, POLY, &[("area".to_string(), id.to_string())], wanted)?,
            Area::Polygon { .. } => return Err(invalid(format!("region `{id}`: the device maps read no polygon yet"))),
            Area::Union { .. } => unreachable!("a leaf region is not a union"),
        };
        let outline = poly.map(|poly| Coverage::parse_poly(&poly).map_err(|e| invalid(format!("{id}.poly: {e}"))));
        match (outline.transpose()?, &mut outlines) {
            (Some(outline), Some(outlines)) => outlines.push(outline),
            _ => outlines = None,
        }
    }
    Ok(outlines)
}

/// The text of the one file that a fetch of `source` with `params` gives, or `None` while the
/// store lacks it; then `wanted` has its fetch.
fn text(
    env: &Env,
    store: &Store,
    source: &str,
    params: &[(String, String)],
    wanted: &mut Vec<Wanted>,
) -> Result<Option<String>, Unplanned> {
    let files = match read(env, store, source, params).map_err(Unplanned::Invalid)? {
        Ok(files) => files,
        Err(fetch) => {
            wanted.push(fetch);
            return Ok(None);
        }
    };
    let [path] = files.values().collect::<Vec<_>>()[..] else {
        return Err(invalid(format!("a fetch of {source} with {params:?} gives {} files, not one", files.len())));
    };
    std::fs::read_to_string(path).map(Some).map_err(|e| invalid(format!("{}: {e}", path.display())))
}

fn invalid(message: String) -> Unplanned {
    Unplanned::Invalid(message)
}

/// A box as an Osmosis `.poly`, the outline that `obc-bake` reads.
pub fn box_poly(bbox: &Bbox) -> String {
    let Bbox { west, south, east, north } = *bbox;
    format!("box\n1\n   {west} {south}\n   {east} {south}\n   {east} {north}\n   {west} {north}\n   {west} {south}\nEND\nEND\n")
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use obc_data::engine::plan::{plan, Plan};
    use obc_data::engine::runs::{Context, Limits, Run as RunLog};
    use obc_data::engine::Request;
    use obc_data::fetch::http::Http;
    use obc_data::store::{sha256_hex, write_atomic, FileRecord, Requested, Snapshot};

    use super::*;

    struct Temp(PathBuf);

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp(name: &str) -> Temp {
        Temp(std::env::temp_dir().join(format!("obc-data-steps-{name}-{}", std::process::id())))
    }

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// The Grimsel box of `data/regions/`, which the leaf edge at 8.388608° cuts in two.
    fn grimsel(glo30: &str) -> (Env, Regions) {
        let pins = BTreeMap::from([(GLO30.to_string(), glo30.to_string()), (TILE_LIST.to_string(), "1".to_string())]);
        let env = Env { name: "test".into(), region: "grimsel".into(), layers: Vec::new(), pins };
        (env, Regions::load(&root()).unwrap())
    }

    /// Add the files `(name, text)` to the record of `source@version`, as a fetch with `params`
    /// gives them.
    fn fetched(store: &Store, source: &str, version: &str, params: &[(String, String)], files: &[(String, String)]) {
        let mut snapshot = store.snapshot(source, version).unwrap().unwrap_or_else(|| Snapshot {
            source: source.into(),
            version: version.into(),
            files: Vec::new(),
        });
        for (name, text) in files {
            let (sha256, file) = (sha256_hex(text.as_bytes()), store.partial("file"));
            write_atomic(&file, text.as_bytes()).unwrap();
            store.insert(&file, &sha256).unwrap();
            let (url, size) = (format!("https://example.org/{name}"), text.len() as u64);
            if snapshot.file(&url).is_none() {
                snapshot.files.push(FileRecord { name: name.clone(), url, size, sha256, retrieved: String::new() });
            }
        }
        store.put_snapshot(&snapshot).unwrap();
        if !params.is_empty() {
            let names = files.iter().map(|(name, _)| name.clone()).collect();
            let requested = Requested { version: version.into(), params: params.to_vec(), files: names };
            store.put_requested(source, &requested).unwrap();
        }
    }

    /// A store whose tile list names the Grimsel tiles in `land`.
    fn with_tile_list(store: &Store, land: &[&str]) {
        let list: String = land.iter().map(|tile| format!("Copernicus_DSM_COG_10_{tile}_00_DEM\n")).collect();
        fetched(store, TILE_LIST, "1", &[], &[("tileList.txt".into(), list)]);
    }

    /// Record the GLO-30 tiles that `steps` read at `version`, each with the text `text(tile)`.
    fn glo30_fetched(store: &Store, steps: &[Step], version: &str, text: impl Fn(&str) -> String) {
        for step in steps {
            let Input::Snapshot { params, .. } = &step.inputs[0] else { unreachable!() };
            let files: Vec<_> = params.iter().map(|(_, tile)| (format!("{tile}/{tile}.tif"), text(tile))).collect();
            fetched(store, GLO30, version, params, &files);
        }
    }

    fn fake(request: &Request) -> Result<(), String> {
        std::fs::create_dir(request.output.join("terrain")).map_err(|e| e.to_string())?;
        std::fs::write(request.output.join("terrain/empty.json"), "[]").map_err(|e| e.to_string())
    }

    fn builds(plan: &Plan) -> Vec<&str> {
        plan.groups.iter().flat_map(|group| &group.builds).map(|build| build.step.as_str()).collect()
    }

    fn tiles(step: &Step) -> Vec<&str> {
        let Input::Snapshot { params, .. } = &step.inputs[0] else { unreachable!() };
        params.iter().map(|(_, tile)| &tile["Copernicus_DSM_COG_10_".len()..][..11]).collect()
    }

    #[test]
    fn a_new_glo30_tile_in_one_leaf_rebuilds_that_leaf_only() {
        let temp = temp("leaves");
        let store = Store::at(temp.0.join("store"));
        with_tile_list(&store, &["N46_00_E007", "N46_00_E008", "N47_00_E007", "N47_00_E008"]);
        let steps = |version: &str| {
            let (env, regions) = grimsel(version);
            let mut steps = Maps.steps(&env, &regions, &store).unwrap();
            steps.iter_mut().for_each(|step| step.run = Run::Rust(fake));
            steps
        };
        let names: Vec<String> = steps("1").iter().map(|step| step.name.clone()).collect();
        assert_eq!(names, ["maps/terrain/0037-0032", "maps/terrain/0037-0033"]);
        let west = "Copernicus_DSM_COG_10_N46_00_E007_00_DEM";
        glo30_fetched(&store, &steps("1"), "1", |tile| format!("{tile} 1"));
        glo30_fetched(&store, &steps("2"), "2", |tile| format!("{tile} {}", if tile == west { 2 } else { 1 }));

        let (root, http) = (root(), Http::new());
        let context = Context { store: &store, root: &root, sources: &[], http: &http, limits: Limits::machine() };
        let first = plan(&store, &root, &steps("1")).unwrap();
        assert_eq!(builds(&first), names);
        let mut run = RunLog::create(&store, "build test").unwrap();
        run.build(&context, &steps("1"), &first).unwrap();
        run.finish(None).unwrap();
        assert_eq!(builds(&plan(&store, &root, &steps("2")).unwrap()), [&names[0]], "the new tile is west of the edge");
    }

    #[test]
    fn a_leaf_reads_the_tiles_of_its_cells_that_the_tile_list_names() {
        let temp = temp("tile-list");
        let store = Store::at(temp.0.join("store"));
        let (env, regions) = grimsel("1");
        let Err(Unplanned::NeedsFetch(wanted)) = Maps.steps(&env, &regions, &store) else { panic!("no tile list") };
        assert_eq!(wanted, [Wanted { source: TILE_LIST.into(), version: Some("1".into()), params: Vec::new() }]);

        // As if N47_00_E007 were sea.
        with_tile_list(&store, &["N46_00_E007", "N46_00_E008", "N47_00_E008"]);
        let steps = Maps.steps(&env, &regions, &store).unwrap();
        assert_eq!(tiles(&steps[0]), ["N46_00_E007", "N46_00_E008", "N47_00_E008"]);
        // The squares of the eastern cells reach no tile west of 8°.
        assert_eq!(tiles(&steps[1]), ["N46_00_E008", "N47_00_E008"]);
    }

    #[test]
    fn no_terrain_step_reads_an_osm_source() {
        let sources = obc_data::sources::parse_sources(include_str!("../../../data/sources.toml")).unwrap();
        let osm =
            |id: &str| sources.iter().any(|source| source.id == id && source.licence.as_deref() == Some("ODbL-1.0"));
        let temp = temp("osm");
        let store = Store::at(temp.0.join("store"));
        with_tile_list(&store, &["N46_00_E007", "N46_00_E008", "N47_00_E007", "N47_00_E008"]);
        let (env, regions) = grimsel("1");
        let steps = Maps.steps(&env, &regions, &store).unwrap();
        let by_name: BTreeMap<&str, &Step> = steps.iter().map(|step| (step.name.as_str(), step)).collect();
        let mut pending: Vec<&Step> = steps.iter().filter(|step| step.name.starts_with("maps/terrain/")).collect();
        assert!(!pending.is_empty());
        while let Some(step) = pending.pop() {
            for input in &step.inputs {
                match input {
                    Input::Snapshot { source, .. } => assert!(!osm(source), "{} reads {source}", step.name),
                    Input::Layer { name, .. } => pending.push(by_name[name.as_str()]),
                }
            }
        }
    }
}
