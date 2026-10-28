//! The producer of `forest`: the canopy of each cell of the zoom 12 tiles at 256 px that the
//! canopy models meet, `12/<x>/<y>.tif` (`specs/obc-data.md`). A version extends the
//! version before: it makes each tile of its region again and keeps every other tile of the version
//! before without an upload. `tools/forest_reference.py fetch` reduces the source rasters of one tile.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use obc_data::prepared::{day, Job, Plan, Prepared, Scope};
use obc_data::product::Wanted;
use obc_data::regions::Bbox;
use obc_data::sources::Source;
use planner_router_build::grid::mercator;

use crate::prepared_tiles::{self, Tile};
use crate::terrain_reference::{area, meets};

/// The id of the prepared source.
pub const ID: &str = "forest";

const ZOOM: u32 = 12;
/// The format of the tiles; a version of format 2 has no tile of `canopy-global`.
pub(crate) const FORMAT: u32 = 3;
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

    fn regional(&self) -> bool {
        true
    }

    /// The models that meet the region, and the leaf type. The tiles of each model in the boxes of
    /// the region are the files at most.
    fn plan(
        &self,
        planning: &obc_data::prepared::Planning,
        _: &Source,
        scope: Option<&Scope>,
        _: &[String],
    ) -> Result<Plan, String> {
        let (boxes, countries) = area(scope)?;
        let wanted = |id: &str| (Wanted { source: id.into(), version: None, params: Vec::new() }, None).into();
        let (mut inputs, mut tiles) = (Vec::new(), BTreeSet::new());
        for (_, model) in models(planning.sources, boxes.as_deref(), &countries) {
            tiles.extend(model_tiles(model, boxes.as_deref())?);
            inputs.push(wanted(&model.id));
        }
        if !inputs.is_empty() {
            inputs.push(wanted(LEAF_TYPE));
        }
        Ok(Plan { inputs, files: Some(tiles.len() as u64) })
    }

    /// Each tile of the region where a model has a pixel, made once and uploaded with the batch of
    /// its tile; every other tile of the version before stays.
    fn prepare(&self, job: &mut Job) -> Result<(), String> {
        let previous = job.extend(self.format())?;
        let (boxes, countries) = area(job.scope.as_ref())?;
        // The models of each tile in the order of `models`: a cell takes the first that has it.
        let mut keys: BTreeMap<(u32, u32), Vec<(String, String)>> = BTreeMap::new();
        for (key, model) in models(job.sources(), boxes.as_deref(), &countries) {
            for tile in model_tiles(model, boxes.as_deref())? {
                keys.entry(tile).or_default().push((key.into(), model.id.clone()));
            }
        }
        let name = |(x, y): &(u32, u32)| format!("12-{x}-{y}");
        let tiles: Vec<Tile> = keys
            .keys()
            .map(|(x, y)| Tile {
                name: name(&(*x, *y)),
                files: vec![(format!("12/{x}/{y}.tif"), format!("12-{x}-{y}.sha256"))],
                marks: Vec::new(),
            })
            .collect();
        let of: BTreeMap<String, String> = keys
            .iter()
            .map(|(tile, models)| {
                (name(tile), models.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>().join(","))
            })
            .collect();
        let digests = prepared_tiles::make(job, &tiles, |job, name| {
            let mut command = (self.tool)(job.root)?;
            command.args(["fetch", &of[name], "--tile", name, "--out"]).arg(&job.work);
            Ok(command)
        })?;
        // A tile outside the region is the tile of the version before.
        if let Some(previous) = &previous {
            let region: BTreeSet<String> = tiles.iter().map(|tile| tile.files[0].0.clone()).collect();
            let kept: Vec<String> =
                previous.files.iter().map(|file| file.path.clone()).filter(|path| !region.contains(path)).collect();
            for path in kept {
                job.reuse(previous, &path)?;
            }
        }
        // The services have no versions of their own: the day of the prepare names what it read.
        let version = day(&job.version).to_string();
        let read: BTreeSet<&String> = keys.values().flatten().map(|(_, id)| id).collect();
        for model in read {
            let made = keys.iter().filter(|(_, models)| models.iter().any(|(_, id)| id == model));
            let made = made.map(|((x, y), _)| format!("12/{x}/{y}.tif"));
            let made = made.filter_map(|path| Some((path.clone(), digests.get(&path)?.clone()))).collect();
            job.input(model, &version, made)?;
        }
        if !digests.is_empty() {
            job.input(LEAF_TYPE, &version, digests)?;
        }
        Ok(())
    }
}

/// The canopy models `canopy-<key>` of `sources` that meet `boxes`, with their keys: the national
/// models, whose key is the lowercase country code, of one of `countries` when the region names
/// some, and then the model `canopy-global` when there are boxes: a region without boxes reads
/// national models only.
pub(crate) fn models<'a>(
    sources: &'a [Source],
    boxes: Option<&'a [Bbox]>,
    countries: &'a [String],
) -> impl Iterator<Item = (&'a str, &'a Source)> {
    let national = sources.iter().filter_map(move |source| {
        let key = source.id.strip_prefix("canopy-").filter(|key| *key != GLOBAL)?;
        (meets(source, boxes) && (countries.is_empty() || countries.contains(&key.to_uppercase())))
            .then_some((key, source))
    });
    let global = sources
        .iter()
        .filter(move |source| boxes.is_some() && source.id == format!("canopy-{GLOBAL}") && meets(source, boxes));
    national.chain(global.map(|source| (GLOBAL, source)))
}

/// The zoom 12 tiles whose inside meets the `extent` of `model` and, with boxes, one of them.
pub(crate) fn model_tiles(model: &Source, boxes: Option<&[Bbox]>) -> Result<BTreeSet<(u32, u32)>, String> {
    let [west, south, east, north] = model.extent.ok_or_else(|| format!("`{}` has no `extent`", model.id))?;
    let extent = Bbox { west, south, east, north };
    let Some(boxes) = boxes else { return Ok(box_tiles(&extent)) };
    Ok(boxes
        .iter()
        .flat_map(|bbox| {
            box_tiles(&Bbox {
                west: extent.west.max(bbox.west),
                south: extent.south.max(bbox.south),
                east: extent.east.min(bbox.east),
                north: extent.north.min(bbox.north),
            })
        })
        .collect())
}

/// The zoom 12 tiles whose inside meets `inside`; none when it is empty.
fn box_tiles(inside: &Bbox) -> BTreeSet<(u32, u32)> {
    if inside.west >= inside.east || inside.south >= inside.north {
        return BTreeSet::new();
    }
    let (left, top) = mercator(inside.west, inside.north, ZOOM);
    let (right, bottom) = mercator(inside.east, inside.south, ZOOM);
    let last = (1u32 << ZOOM) - 1;
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
    use obc_data::target::Target;

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

    /// The tool as a script: a tile with a file `none-<tile>` has no pixel, and every other tile
    /// gets its name as bytes and as digest.
    const TOOL: &str = r#"[ -e "none-$4" ] && { touch "$6/$4.none"; exit 0; }
t="${4#12-}"; mkdir -p "$6/12/${t%-*}" && echo "$4" > "$6/12/${t%-*}/${t#*-}.tif" && echo "pixels of $4" > "$6/$4.sha256"
"#;

    fn script(_: &Path) -> Result<Command, String> {
        let mut command = Command::new("sh");
        command.arg("tool.sh");
        Ok(command)
    }

    /// A Swiss box from `west` to `east` over the extent of the model.
    fn scope(id: &str, west: f64, east: f64) -> Scope {
        let text =
            format!("name = \"{id}\"\nkind = \"box\"\nbox = [{west}, 46.5, {east}, 46.6]\ncountries = [\"CH\"]\n");
        Scope { region: id.into(), definitions: [(id.into(), text)].into() }
    }

    #[test]
    fn the_global_model_meets_every_country_and_comes_after_the_national_ones() {
        let rows = |id: &str, extent: &str| {
            format!("[[source]]\nid = \"{id}\"\nkind = \"data\"\nlicence = \"CC0-1.0\"\nattribution = \"Test\"\nfetch = {{ kind = \"capture\", url = \"https://example.org\" }}\nextent = {extent}\nversion = \"date\"\nkeep = \"prepared\"\nredistribute = true\n")
        };
        let text =
            [rows("canopy-global", "[-180.0, -85.0, 180.0, 85.0]"), rows("canopy-ch", "[5.9, 45.8, 10.5, 47.9]")];
        let temp = temp("forest-models");
        std::fs::create_dir_all(temp.0.join("data")).unwrap();
        std::fs::write(temp.0.join("data/sources.toml"), text.concat()).unwrap();
        let sources = Registry::load(&temp.0).unwrap().sources;
        let keys = |countries: &[&str]| {
            let countries: Vec<String> = countries.iter().map(|code| code.to_string()).collect();
            let bbox = Bbox { west: 7.0, south: 46.0, east: 9.0, north: 48.0 };
            models(&sources, Some(&[bbox]), &countries).map(|(key, _)| key.to_string()).collect::<Vec<_>>()
        };
        assert_eq!(keys(&["CH", "DE"]), ["ch", "global"]);
        assert_eq!(keys(&["DE"]), ["global"]);
    }

    #[test]
    fn a_union_reads_the_tiles_of_each_box_and_none_between_them() {
        let temp = temp("forest-union");
        std::fs::create_dir_all(temp.0.join("data")).unwrap();
        // `canopy-li` lies between the two boxes.
        let gap = "[[source]]\nid = \"canopy-li\"\nkind = \"data\"\nlicence = \"CC0-1.0\"\nattribution = \"Test\"\nfetch = { kind = \"capture\", url = \"https://example.org\" }\nextent = [8.08, 46.5, 8.12, 46.6]\nversion = \"date\"\nkeep = \"prepared\"\nredistribute = true\n";
        std::fs::write(temp.0.join("data/sources.toml"), format!("{SOURCES}\n{gap}")).unwrap();
        let sources = Registry::load(&temp.0).unwrap().sources;
        let model = sources.iter().find(|source| source.id == "canopy-ch").unwrap();
        let boxed = |id: &str, west: f64, east: f64| {
            (id.to_string(), format!("name = \"{id}\"\nkind = \"box\"\nbox = [{west}, 46.5, {east}, 46.6]\n"))
        };
        let union = "name = \"Both\"\nkind = \"union\"\nunion = [\"west\", \"east\"]\n".to_string();
        let definitions = [boxed("west", 8.0, 8.02), boxed("east", 8.18, 8.2), ("both".into(), union)];
        let scope = Scope { region: "both".into(), definitions: definitions.into() };
        let (boxes, _) = area(Some(&scope)).unwrap();
        let keys: Vec<&str> = models(&sources, boxes.as_deref(), &[]).map(|(key, _)| key).collect();
        assert_eq!(keys, ["ch"]);
        let tiles = model_tiles(model, boxes.as_deref()).unwrap();
        // The extent meets three columns of zoom 12 tiles; each box meets only the outer one at its side.
        let columns: BTreeSet<u32> = tiles.iter().map(|(x, _)| *x).collect();
        let all: BTreeSet<u32> = model_tiles(model, None).unwrap().iter().map(|(x, _)| *x).collect();
        assert_eq!(all.len(), 3);
        assert_eq!(columns, [*all.first().unwrap(), *all.last().unwrap()].into());
    }

    #[test]
    fn a_version_makes_the_tiles_of_its_region_and_keeps_the_others_of_the_version_before() {
        let temp = temp("forest");
        let root = temp.0.join("repository");
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/sources.toml"), SOURCES).unwrap();
        std::fs::write(root.join("tool.sh"), TOOL).unwrap();
        let (store, target) = (Store::at(temp.0.join("store")), Target::Dir(temp.0.join("target")));
        let sources = Registry::load(&root).unwrap().sources;
        let (producer, http) = (ForestReference { tool: script }, Http::new());
        let version = |version: &str, scope: Scope| {
            let mut run = Run::create(&store, ["source", "prepare", ID]).unwrap();
            let opening =
                Opening { root: &root, store: &store, http: &http, sources: &sources, target: ("t", &target) };
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
            obc_data::prepared::complete(&target, &manifest).unwrap();
            run.flush().unwrap();
            let status: serde_json::Value = serde_json::from_slice(&std::fs::read(run.status_file()).unwrap()).unwrap();
            // Each tile that ended while others ran keeps what it used.
            for step in status["steps"].as_array().unwrap() {
                assert!(step["cpu_ms"].is_u64() && step["peak_rss_bytes"].is_u64(), "{step}");
            }
            manifest
        };
        let model = sources.iter().find(|source| source.id == "canopy-ch").unwrap();
        let path = |(x, y): &(u32, u32)| format!("12/{x}/{y}.tif");
        let tiles = |west: f64, east: f64| {
            let bbox = Bbox { west, south: 46.5, east, north: 46.6 };
            model_tiles(model, Some(&[bbox])).unwrap().into_iter().collect::<Vec<_>>()
        };
        // 8.0–8.2 °E meets three columns of zoom 12 tiles; each half of it meets two.
        let (west, east) = (tiles(8.0, 8.1), tiles(8.1, 8.2));
        assert_eq!((west.len(), east.len(), tiles(8.0, 8.2).len()), (4, 4, 6), "{west:?} {east:?}");
        let (x, y) = west[0];
        std::fs::write(root.join(format!("none-12-{x}-{y}")), "").unwrap();

        let first = version("2026-10-01", scope("west", 8.0, 8.1));
        let made: Vec<String> = first.files.iter().map(|file| file.path.clone()).collect();
        assert_eq!(made, west[1..].iter().map(path).collect::<Vec<_>>());

        let second = version("2026-10-02", scope("east", 8.1, 8.2));
        let mut held: Vec<String> = west[1..].iter().chain(&east).map(path).collect();
        held.sort();
        held.dedup();
        assert_eq!(second.files.iter().map(|file| file.path.clone()).collect::<Vec<_>>(), held);
        let kept: Vec<&String> = second.reused.keys().collect();
        let only_west: Vec<String> = west[1..].iter().filter(|tile| !east.contains(tile)).map(path).collect();
        assert_eq!(kept, only_west.iter().collect::<Vec<_>>(), "a tile of the region is made again");
        assert!(second.reused.values().all(|version| version == "2026-10-01"));
        assert_eq!(second.extends, [first.scope]);
    }
}
