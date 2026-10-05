//! The device maps. Each layer covers one leaf of the `2^23` grid that the region touches:
//! `maps/terrain/<i>-<j>` holds the terrain cells of the region in leaf `(i, j)`.

use std::collections::{BTreeMap, BTreeSet};

use obc_bake::coverage::Coverage;
use obc_data::engine::plan::Fetch;
use obc_data::engine::{snapshot_files, Code, Input, Run, Step};
use obc_data::env::Env;
use obc_data::product::{Product, Unplanned};
use obc_data::regions::{Area, Bbox, Regions};
use obc_data::store::Store;
use obc_dem::bake::{V1_CELL_LOG2, V1_POSTING_LOG2};
use obc_dem::step::GLO30;
use obc_pack::grid::{id_width, CellId};

/// The cell of the planet bake, and of every device-map layer.
const LEAF_LOG2: u32 = obc_bake::planet::SOURCE_LEAF_LOG2;
const POLY: &str = "geofabrik-poly";

pub struct Maps;

impl Product for Maps {
    fn name(&self) -> &'static str {
        "maps"
    }

    fn steps(&self, env: &Env, regions: &Regions, store: &Store) -> Result<Vec<Step>, Unplanned> {
        let glo30 = env.version(GLO30).map_err(Unplanned::Invalid)?;
        let cells: BTreeSet<CellId> =
            outlines(env, regions, store)?.iter().flat_map(|outline| outline.cells(V1_CELL_LOG2.into())).collect();
        let mut leaves: BTreeMap<(i64, i64), Vec<CellId>> = BTreeMap::new();
        let shift = LEAF_LOG2 - u32::from(V1_CELL_LOG2);
        for cell in cells {
            leaves.entry((cell.i >> shift, cell.j >> shift)).or_default().push(cell);
        }
        Ok(leaves.into_iter().map(|(leaf, cells)| terrain(leaf, &cells, glo30)).collect())
    }
}

/// The terrain cells of one leaf, from the GLO-30 tiles that their squares reach.
fn terrain((i, j): (i64, i64), cells: &[CellId], glo30: &str) -> Step {
    let bbox = obc_bake::terrain::source_bbox(cells.iter().copied()).expect("a leaf has a cell");
    let tiles = obc_dem::fetch::tiles_for(bbox).iter().map(|tile| ("tile".to_string(), tile.stem())).collect();
    let width = id_width(LEAF_LOG2);
    Step {
        name: format!("maps/terrain/{i:0width$}-{j:0width$}"),
        inputs: vec![Input::Snapshot { source: GLO30.into(), version: glo30.into(), params: tiles, files: Vec::new() }],
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

/// The outline of each region that `env.region` resolves to. A Geofabrik area reads its `.poly`.
fn outlines(env: &Env, regions: &Regions, store: &Store) -> Result<Vec<Coverage>, Unplanned> {
    let mut polys = Vec::new();
    let mut fetches = Vec::new();
    for id in regions.leaves(&env.region).map_err(Unplanned::Invalid)? {
        let poly = match &regions.get(id).expect("a leaf region exists").area {
            Area::Box { bbox } => box_poly(bbox),
            Area::Geofabrik => {
                let version = env.version(POLY).map_err(Unplanned::Invalid)?;
                let params = vec![("area".to_string(), id.to_string())];
                match snapshot_files(store, POLY, version, &params, &[]).map_err(Unplanned::Invalid)? {
                    Some(files) => {
                        let path = files.values().next().ok_or(format!("{POLY}@{version} has no file for {id}"));
                        let path = path.map_err(Unplanned::Invalid)?;
                        std::fs::read_to_string(path)
                            .map_err(|e| Unplanned::Invalid(format!("{}: {e}", path.display())))?
                    }
                    None => {
                        let version = version.to_string();
                        fetches.push(Fetch { source: POLY.into(), version, params, files: Vec::new(), bytes: None });
                        continue;
                    }
                }
            }
            Area::Polygon { .. } => {
                return Err(Unplanned::Invalid(format!("region `{id}`: the device maps read no polygon file yet")))
            }
            Area::Union { .. } => unreachable!("a leaf region is not a union"),
        };
        polys.push((id, poly));
    }
    if !fetches.is_empty() {
        return Err(Unplanned::NeedsFetch(fetches));
    }
    let outline = |(id, poly): (&str, String)| Coverage::parse_poly(&poly).map_err(|e| format!("{id}.poly: {e}"));
    polys.into_iter().map(outline).collect::<Result<_, _>>().map_err(Unplanned::Invalid)
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

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// The Grimsel box of `data/regions/`, which the leaf edge at 8.388608° cuts in two.
    fn grimsel(glo30: &str) -> (Env, Regions) {
        let pins = BTreeMap::from([(GLO30.to_string(), glo30.to_string())]);
        let env = Env { name: "test".into(), region: "grimsel".into(), layers: Vec::new(), pins };
        (env, Regions::load(&root()).unwrap())
    }

    /// Record the GLO-30 tiles that `steps` read at `version`, each with the bytes `bytes(tile)`,
    /// as their fetches would.
    fn fetched(store: &Store, steps: &[Step], version: &str, bytes: impl Fn(&str) -> String) {
        let mut files = BTreeMap::new();
        for step in steps {
            let Input::Snapshot { params, .. } = &step.inputs[0] else { unreachable!() };
            let mut names = Vec::new();
            for (_, tile) in params {
                let (name, body) = (format!("{tile}/{tile}.tif"), bytes(tile));
                let (sha256, file) = (sha256_hex(body.as_bytes()), store.partial(tile));
                write_atomic(&file, body.as_bytes()).unwrap();
                store.insert(&file, &sha256).unwrap();
                let (url, size) = (format!("https://example.org/{name}"), body.len() as u64);
                let record = FileRecord { name: name.clone(), url, size, sha256, retrieved: String::new() };
                files.insert(name.clone(), record);
                names.push(name);
            }
            let requested = Requested { version: version.into(), params: params.clone(), files: names };
            store.put_requested(GLO30, &requested).unwrap();
        }
        let files = files.into_values().collect();
        store.put_snapshot(&Snapshot { source: GLO30.into(), version: version.into(), files }).unwrap();
    }

    fn fake(request: &Request) -> Result<(), String> {
        std::fs::create_dir(request.output.join("terrain")).map_err(|e| e.to_string())?;
        std::fs::write(request.output.join("terrain/empty.json"), "[]").map_err(|e| e.to_string())
    }

    fn builds(plan: &Plan) -> Vec<&str> {
        plan.groups.iter().flat_map(|group| &group.builds).map(|build| build.step.as_str()).collect()
    }

    #[test]
    fn a_new_glo30_tile_in_one_leaf_rebuilds_that_leaf_only() {
        let temp = Temp(std::env::temp_dir().join(format!("obc-data-steps-leaves-{}", std::process::id())));
        let store = Store::at(temp.0.join("store"));
        let steps = |version: &str| {
            let (env, regions) = grimsel(version);
            let mut steps = Maps.steps(&env, &regions, &store).unwrap();
            steps.iter_mut().for_each(|step| step.run = Run::Rust(fake));
            steps
        };
        let names: Vec<String> = steps("1").iter().map(|step| step.name.clone()).collect();
        assert_eq!(names, ["maps/terrain/0037-0032", "maps/terrain/0037-0033"]);
        let west = "Copernicus_DSM_COG_10_N46_00_E007_00_DEM";
        fetched(&store, &steps("1"), "1", |tile| format!("{tile} 1"));
        fetched(&store, &steps("2"), "2", |tile| format!("{tile} {}", if tile == west { 2 } else { 1 }));

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
    fn no_terrain_step_reads_an_osm_source() {
        let sources = obc_data::sources::parse_sources(include_str!("../../../data/sources.toml")).unwrap();
        let osm =
            |id: &str| sources.iter().any(|source| source.id == id && source.licence.as_deref() == Some("ODbL-1.0"));
        let (env, regions) = grimsel("1");
        let steps = Maps.steps(&env, &regions, &Store::at(std::env::temp_dir().join("obc-data-steps-unused"))).unwrap();
        let by_name: BTreeMap<&str, &Step> = steps.iter().map(|step| (step.name.as_str(), step)).collect();
        let mut pending: Vec<&Step> = steps.iter().filter(|step| step.name.starts_with("maps/terrain/")).collect();
        assert!(!pending.is_empty());
        while let Some(step) = pending.pop() {
            for input in &step.inputs {
                match input {
                    Input::Snapshot { source, .. } => assert!(!osm(source), "{} reads {source}", step.name),
                    Input::Layer(name) => pending.push(by_name[name.as_str()]),
                }
            }
        }
    }
}
