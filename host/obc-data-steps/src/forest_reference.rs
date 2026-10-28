//! The producer of `forest`: the canopy of each cell of the zoom 12 tiles at 256 px, one file
//! `10/<x>/<y>.tif` per zoom 10 block (`specs/obc-data.md`). A version makes each block of its
//! region from every canopy model of its tiles, so the pixels of a block do not depend on the
//! region, and keeps every other block of the version before without a copy.
//! `tools/forest_reference.py fetch` reduces the source rasters of one block.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use obc_bake::coverage::Coverage;
use obc_data::prepared::{Job, Plan, Prepared, Scope};
use obc_data::product::Wanted;
use obc_data::regions::{Area, Bbox};
use obc_data::sources::Source;
use planner_router_build::grid::mercator;

use crate::device::box_poly;
use crate::prepared_tiles::{self, outlines, poly, read_on_day, regions, reuse, Tile};

/// The id of the prepared source.
pub const ID: &str = "forest";

const ZOOM: u32 = 12;
/// The zoom of a block: 4 × 4 zoom 12 tiles.
const BLOCK_ZOOM: u32 = 10;
/// The format of the blocks.
pub(crate) const FORMAT: u32 = 4;
/// The key of the model of every country, which a cell reads only where no national model has it.
const GLOBAL: &str = "global";

/// What the tool reads besides the model of a tile.
pub(crate) const LEAF_TYPE: &str = "hrl-dominant-leaf-type";

pub struct ForestReference {
    /// `tools/forest_reference.py`, as a command in the repository root.
    pub tool: fn(&Path) -> Result<Command, String>,
}

pub const FOREST_REFERENCE: ForestReference = ForestReference { tool };

fn tool(root: &Path) -> Result<Command, String> {
    let mut command = Command::new("uv");
    command.args(["run", "--locked", "--offline", "--no-default-groups", "--no-python-downloads"]);
    command.args(["--group", "planner-snow", "--group", "forest-reference", "python", "-m", "tools.forest_reference"]);
    command.env("UV_PYTHON", obc_data::engine::python_executable(root)?).env("UV_NO_SYNC", "0");
    Ok(command)
}

impl Prepared for ForestReference {
    fn name(&self) -> &'static str {
        ID
    }

    fn format(&self) -> u32 {
        FORMAT
    }

    fn revision(&self) -> u32 {
        1
    }

    /// The models of the blocks of a region of boxes, or every model, the leaf type, and the
    /// polygons of the Geofabrik leaves. The blocks of a region of boxes are the files.
    fn plan(
        &self,
        planning: &obc_data::prepared::Planning,
        _: &Source,
        scope: Option<&Scope>,
        _: &[String],
    ) -> Result<Plan, String> {
        let wanted = |id: &str| (Wanted { source: id.into(), version: None, params: Vec::new() }, None).into();
        let mut inputs = Vec::new();
        let blocks = match scope {
            None => Some(national(planning.sources)?),
            Some(scope) => {
                let (regions, id) = regions(scope)?;
                let mut blocks = Some(BTreeSet::new());
                for leaf in regions.leaves(&id)? {
                    match &regions.get(leaf).expect("a leaf is a region").area {
                        Area::Box { bbox } => {
                            if let Some(blocks) = &mut blocks {
                                blocks.extend(region(&Coverage::parse_poly(&box_poly(bbox))?)?);
                            }
                        }
                        Area::Geofabrik { areas } => {
                            inputs.extend(areas.iter().map(|area| (poly(area, None), None).into()));
                            blocks = None;
                        }
                        Area::Union { .. } => {}
                    }
                }
                blocks
            }
        };
        let mut read: BTreeSet<&str> = BTreeSet::new();
        for block in blocks.iter().flatten() {
            read.extend(block_models(planning.sources, *block).values().flatten().map(|(_, model)| model.id.as_str()));
        }
        if blocks.is_none() {
            read.extend(models(planning.sources).map(|(_, model)| model.id.as_str()));
        }
        inputs.extend(read.iter().map(|id| wanted(id)));
        if !read.is_empty() {
            inputs.push(wanted(LEAF_TYPE));
        }
        Ok(Plan { inputs, files: blocks.map(|blocks| blocks.len() as u64), work_bytes: 0 })
    }

    /// Each block of the region, made once and moved into the home with the batch of its block;
    /// every other block of the version before stays.
    fn prepare(&self, job: &mut Job) -> Result<(), String> {
        let previous = job.extend(self.format())?;
        let blocks = match outlines(job)? {
            Some(outlines) => {
                let mut blocks = BTreeSet::new();
                for outline in &outlines {
                    blocks.extend(region(outline)?);
                }
                blocks
            }
            None => national(job.sources())?,
        };
        let name = |(x, y): (u32, u32)| format!("10-{x}-{y}");
        let mut tiles = Vec::new();
        // The arguments of the program of each block, and the models that it reads.
        let mut args: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut read: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for block in blocks {
            let of = block_models(job.sources(), block);
            if of.is_empty() {
                continue;
            }
            let (name, (x, y)) = (name(block), block);
            let path = format!("10/{x}/{y}.tif");
            for (_, model) in of.values().flatten() {
                read.entry(model.id.clone()).or_default().insert(path.clone());
            }
            let keys = |models: &Vec<(&str, &Source)>| models.iter().map(|(key, _)| *key).collect::<Vec<_>>().join(",");
            args.insert(
                name.clone(),
                of.iter().map(|((x, y), models)| format!("12-{x}-{y}={}", keys(models))).collect(),
            );
            tiles.push(Tile { name: name.clone(), files: vec![(path, format!("{name}.sha256"))], marks: Vec::new() });
        }
        let digests = prepared_tiles::make(job, &tiles, |job, name| {
            let mut command = (self.tool)(job.root)?;
            command.arg("fetch").args(&args[name]).args(["--block", name, "--out"]).arg(&job.work);
            Ok(command)
        })?;
        // A block outside the region is the block of the version before.
        let region: BTreeSet<&str> = tiles.iter().map(|tile| tile.files[0].0.as_str()).collect();
        reuse(job, previous.as_ref(), |path| !region.contains(path))?;
        for (model, paths) in read {
            let made = paths.iter().filter_map(|path| Some((path.clone(), digests.get(path)?.clone())));
            read_on_day(job, &model, made.collect())?;
        }
        if !digests.is_empty() {
            read_on_day(job, LEAF_TYPE, digests)?;
        }
        Ok(())
    }
}

/// The canopy models `canopy-<key>` of `sources` with their keys: the national models, whose key is
/// the lowercase country code, and then the model `canopy-global`.
fn models(sources: &[Source]) -> impl Iterator<Item = (&str, &Source)> {
    let national = sources.iter().filter_map(|source| {
        let key = source.id.strip_prefix("canopy-").filter(|key| *key != GLOBAL)?;
        Some((key, source))
    });
    let global = sources.iter().filter(|source| source.id == format!("canopy-{GLOBAL}"));
    national.chain(global.map(|source| (GLOBAL, source)))
}

/// The models of each zoom 12 tile of `block` that the `extent` of a model meets, in the order of
/// [`models`]; a tile that no model meets is not named.
pub(crate) fn block_models(sources: &[Source], (x, y): (u32, u32)) -> BTreeMap<(u32, u32), Vec<(&str, &Source)>> {
    let mut tiles = BTreeMap::new();
    for tile in (4 * x..4 * x + 4).flat_map(|x| (4 * y..4 * y + 4).map(move |y| (x, y))) {
        let held: Vec<(&str, &Source)> = models(sources).filter(|(_, model)| model.meets(square(ZOOM, tile))).collect();
        if !held.is_empty() {
            tiles.insert(tile, held);
        }
    }
    tiles
}

/// The blocks that `outline` meets. Only a block that a lattice cell of the boundary overlaps can
/// hold a part of the boundary, so only such a block needs the intersection; the centre of any
/// other block says whether it is inside.
pub(crate) fn region(outline: &Coverage) -> Result<BTreeSet<(u32, u32)>, String> {
    let degrees = |udeg: i64| udeg as f64 / 1e6;
    let bbox = |(west, south, east, north): (i64, i64, i64, i64)| Bbox {
        west: degrees(west),
        south: degrees(south),
        east: degrees(east),
        north: degrees(north),
    };
    let crossed: BTreeSet<(u32, u32)> = outline
        .boundary_cells(obc_dem::reference::TILE_LOG2.into())
        .into_iter()
        .flat_map(|cell| tiles(&bbox(cell.square()), BLOCK_ZOOM))
        .collect();
    let mut found = BTreeSet::new();
    for block in tiles(&bbox(outline.bbox()), BLOCK_ZOOM) {
        let [west, south, east, north] = square(BLOCK_ZOOM, block);
        let inside = match crossed.contains(&block) {
            true => {
                let square = Coverage::parse_poly(&box_poly(&Bbox { west, south, east, north }))?;
                outline.intersection(&square)?.is_some()
            }
            false => outline.contains(((south + north) / 2.0 * 1e6) as i64, ((west + east) / 2.0 * 1e6) as i64),
        };
        if inside {
            found.insert(block);
        }
    }
    Ok(found)
}

/// The blocks of a version of everything: those with a tile that the `extent` of a national model
/// meets.
fn national(sources: &[Source]) -> Result<BTreeSet<(u32, u32)>, String> {
    let mut found = BTreeSet::new();
    for (key, model) in models(sources).filter(|(key, _)| *key != GLOBAL) {
        let [west, south, east, north] = model.extent.ok_or_else(|| format!("`canopy-{key}` has no `extent`"))?;
        found.extend(tiles(&Bbox { west, south, east, north }, ZOOM).into_iter().map(|(x, y)| (x >> 2, y >> 2)));
    }
    Ok(found)
}

/// The path of `block` in a version.
pub(crate) fn path((x, y): (u32, u32)) -> String {
    format!("{BLOCK_ZOOM}/{x}/{y}.tif")
}

/// West, south, east and north of tile `(x, y)` of `zoom`, in degrees.
fn square(zoom: u32, (x, y): (u32, u32)) -> [f64; 4] {
    let n = f64::from(1u32 << zoom);
    let lon = |x: u32| f64::from(x) / n * 360.0 - 180.0;
    let lat = |y: u32| (std::f64::consts::PI * (1.0 - 2.0 * f64::from(y) / n)).sinh().atan().to_degrees();
    [lon(x), lat(y + 1), lon(x + 1), lat(y)]
}

/// The tiles of `zoom` whose inside meets `inside`; none when it is empty.
fn tiles(inside: &Bbox, zoom: u32) -> BTreeSet<(u32, u32)> {
    if inside.west >= inside.east || inside.south >= inside.north {
        return BTreeSet::new();
    }
    let (left, top) = mercator(inside.west, inside.north, zoom);
    let (right, bottom) = mercator(inside.east, inside.south, zoom);
    let last = (1u32 << zoom) - 1;
    let span = |from: f64, to: f64| (from.floor() as u32).min(last)..=((to.ceil() as u32).max(1) - 1).min(last);
    span(left, right).flat_map(|x| span(top, bottom).map(move |y| (x, y))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::tests::temp;
    use obc_data::engine::runs::Run;
    use obc_data::fetch::http::Http;
    use obc_data::prepared::{Manifest, Opening};
    use obc_data::sources::Registry;
    use obc_data::store::Store;

    const SOURCES: &str = r#"
        [[source]]
        id = "canopy-ch"
        kind = "data"
        licence = "CC0-1.0"
        attribution = "Test"
        fetch = { kind = "capture", url = "https://example.org/vhm" }
        extent = [8.0, 46.5, 8.2, 46.6]
        version = "date"
        keep = "prepared"
        redistribute = true

        [[source]]
        id = "forest"
        kind = "data"
        licence = "CC0-1.0"
        attribution = "Test"
        fetch = { kind = "prepared" }
        version = "date"
        keep = "prepared"
        redistribute = true
    "#;

    /// The tool as a script: it logs the tiles of each block to `tiles`, and writes the name of the
    /// block as its bytes and its digest.
    const TOOL: &str = r#"shift; while [ "$1" != --block ]; do echo "$1" >> tiles; shift; done
b="$2"; t="${b#10-}"; mkdir -p "$4/10/${t%-*}" && echo "$b" > "$4/10/${t%-*}/${t#*-}.tif" && echo "pixels of $b" > "$4/$b.sha256"
"#;

    fn script(_: &Path) -> Result<Command, String> {
        let mut command = Command::new("sh");
        command.arg("tool.sh");
        Ok(command)
    }

    /// A box from `west` to `east` over the extent of the model, in a region of Germany: the
    /// countries of a region do not choose the models.
    fn scope(id: &str, west: f64, east: f64) -> Scope {
        let text =
            format!("name = \"{id}\"\nkind = \"box\"\nbox = [{west}, 46.5, {east}, 46.6]\ncountries = [\"DE\"]\n");
        Scope { region: id.into(), definitions: [(id.into(), text)].into() }
    }

    #[test]
    fn the_global_model_meets_every_tile_and_comes_after_the_national_ones() {
        let rows = |id: &str, extent: &str| {
            format!("[[source]]\nid = \"{id}\"\nkind = \"data\"\nlicence = \"CC0-1.0\"\nattribution = \"Test\"\nfetch = {{ kind = \"capture\", url = \"https://example.org\" }}\nextent = {extent}\nversion = \"date\"\nkeep = \"prepared\"\nredistribute = true\n")
        };
        let text =
            [rows("canopy-global", "[-180.0, -85.0, 180.0, 85.0]"), rows("canopy-ch", "[5.9, 45.8, 10.5, 47.9]")];
        let temp = temp("forest-models");
        std::fs::create_dir_all(temp.0.join("data")).unwrap();
        std::fs::write(temp.0.join("data/sources.toml"), text.concat()).unwrap();
        let sources = Registry::load(&temp.0).unwrap().sources;
        let keys = |block| -> Vec<Vec<&str>> {
            block_models(&sources, block).values().map(|models| models.iter().map(|(key, _)| *key).collect()).collect()
        };
        assert_eq!(keys((534, 362)), vec![vec!["ch", "global"]; 16], "a Swiss block");
        assert_eq!(keys((540, 340)), vec![vec!["global"]; 16], "a block of Germany");
    }

    #[test]
    fn the_blocks_of_a_polygon_are_those_that_it_meets() {
        // An L of two squares of 1°: the bounding box holds a block in its empty corner.
        let poly = "l\n1\n   8.0 46.0\n   10.0 46.0\n   10.0 47.0\n   9.0 47.0\n   9.0 48.0\n   8.0 48.0\n   8.0 46.0\nEND\nEND\n";
        let blocks = region(&Coverage::parse_poly(poly).unwrap()).unwrap();
        let corner = tiles(&Bbox { west: 9.6, south: 47.6, east: 9.7, north: 47.7 }, BLOCK_ZOOM);
        let inside = tiles(&Bbox { west: 8.5, south: 47.5, east: 8.6, north: 47.6 }, BLOCK_ZOOM);
        assert!(corner.iter().all(|block| !blocks.contains(block)), "{corner:?}");
        assert!(inside.iter().all(|block| blocks.contains(block)), "{inside:?}");
        let all = tiles(&Bbox { west: 8.0, south: 46.0, east: 10.0, north: 48.0 }, BLOCK_ZOOM);
        assert!(blocks.is_subset(&all) && blocks.len() < all.len());
    }

    #[test]
    fn a_version_makes_the_whole_blocks_of_its_region_and_keeps_the_others_of_the_version_before() {
        let temp = temp("forest");
        let root = temp.0.join("repository");
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/sources.toml"), SOURCES).unwrap();
        std::fs::write(root.join("tool.sh"), TOOL).unwrap();
        let (store, archive) = (Store::at(temp.0.join("store")), temp.0.join("archive"));
        let sources = Registry::load(&root).unwrap().sources;
        let (producer, http) = (ForestReference { tool: script }, Http::new());
        let version = |version: &str, scope: Scope| {
            let _ = std::fs::remove_file(root.join("tiles"));
            let mut run = Run::create(&store, ["source", "prepare", ID]).unwrap();
            let opening = Opening { root: &root, store: &store, http: &http, sources: &sources, home: &archive };
            let mut job = Job::open(opening, (ID, version), Some(scope), &[], &mut run).unwrap();
            producer.prepare(&mut job).unwrap();
            let manifest = Manifest {
                source: ID.into(),
                version: version.into(),
                format: producer.format(),
                revision: producer.revision(),
                inputs: Vec::new(),
                files: job.files().cloned().collect(),
                reused: job.reused().clone(),
                scope: job.scope.clone(),
                extends: job.extends().to_vec(),
                languages: Vec::new(),
                licence: None,
                attribution: None,
                machine: "test".into(),
                time: format!("{version}T00:00:00Z"),
                cost: obc_data::prepared::Cost { wall_ms: 0, peak_rss_bytes: None },
            };
            obc_data::prepared::complete(&obc_data::prepared::folder(&archive, ID), &manifest).unwrap();
            run.flush().unwrap();
            let status: serde_json::Value = serde_json::from_slice(&std::fs::read(run.status_file()).unwrap()).unwrap();
            // Each block that ended while others ran keeps what it used.
            for step in status["steps"].as_array().unwrap() {
                assert!(step["cpu_ms"].is_u64() && step["peak_rss_bytes"].is_u64(), "{step}");
            }
            let mut tiles: Vec<String> =
                std::fs::read_to_string(root.join("tiles")).unwrap().lines().map(String::from).collect();
            tiles.sort();
            (manifest, tiles)
        };
        let paths = |manifest: &Manifest| manifest.files.iter().map(|file| file.path.clone()).collect::<Vec<_>>();
        // The extent holds the zoom 12 tiles 2139 to 2141 by 1447 and 1448, in the blocks 534 and
        // 535 by 361 and 362.
        let (first, tiles) = version("2026-10-01", scope("west", 8.0, 8.05));
        assert_eq!(paths(&first), ["10/534/361.tif", "10/534/362.tif"]);
        assert_eq!(tiles, ["12-2139-1447=ch", "12-2139-1448=ch"]);

        // The east box meets only the tiles of column 2141; its blocks hold column 2140 too.
        let (second, tiles) = version("2026-10-02", scope("east", 8.18, 8.2));
        assert_eq!(tiles, ["12-2140-1447=ch", "12-2140-1448=ch", "12-2141-1447=ch", "12-2141-1448=ch"]);
        assert_eq!(paths(&second), ["10/534/361.tif", "10/534/362.tif", "10/535/361.tif", "10/535/362.tif"]);
        let kept: Vec<&String> = second.reused.keys().collect();
        assert_eq!(kept, ["10/534/361.tif", "10/534/362.tif"], "a block outside the region stays");
        assert!(second.reused.values().all(|version| version == "2026-10-01"));
        assert_eq!(second.extends, [first.scope]);
    }
}
