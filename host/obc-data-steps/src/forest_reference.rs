//! The producer of `forest-reference/<key>`: the canopy of each cell of the zoom 12 tiles at 256 px
//! that meet the `extent` of the source, `12/<x>/<y>.tif`, and their `index.json`
//! (`specs/obc-data.md`). `tools/forest_reference.py fetch` reduces the source rasters of one tile.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use obc_data::prepared::{Job, Plan, Prepared, Scope};
use obc_data::product::Wanted;
use obc_data::regions::Bbox;
use obc_data::settings::Settings;
use obc_data::sources::Source;
use planner_router_build::grid::mercator;

const ZOOM: u32 = 12;
/// The tiles of one upload.
const BATCH: usize = 64;

pub struct ForestReference {
    /// `tools/forest_reference.py`, as a command in the repository root.
    pub tool: fn(&Path) -> Result<Command, String>,
}

pub const FOREST_REFERENCE: ForestReference = ForestReference { tool };

fn tool(root: &Path) -> Result<Command, String> {
    let mut command = Command::new("uv");
    command.args(["run", "--locked", "--offline", "--no-default-groups", "--no-python-downloads"]);
    command.args(["--group", "planner-snow", "--group", "planner-sun", "python", "-m", "tools.forest_reference"]);
    command.env("UV_PYTHON", obc_data::engine::python_executable(root)?).env("UV_NO_SYNC", "0");
    Ok(command)
}

/// The sources that the tool reads for `key`: the canopy, then the leaf type.
fn reads(key: &str) -> Result<&'static [&'static str], String> {
    match key {
        "ch" => Ok(&["vhm-lidar-ch", "hrl-dominant-leaf-type"]),
        _ => Err(format!("forest-reference/{key}: `tools/forest_reference.py` has no source for `{key}`")),
    }
}

impl Prepared for ForestReference {
    fn name(&self) -> &'static str {
        "forest-reference"
    }

    fn format(&self) -> u32 {
        1
    }

    fn revision(&self) -> u32 {
        1
    }

    /// The sources that the tool reads. The tiles of the `extent`, in the box of a scope, and
    /// `index.json` are the files at most.
    fn plan(
        &self,
        _: &obc_data::prepared::Planning,
        source: &Source,
        scope: Option<&Scope>,
        _: &[String],
    ) -> Result<Plan, String> {
        let reads = reads(key(source))?;
        let tiles = tiles(source, scope)?;
        let wanted = |id: &&str| Wanted { source: id.to_string(), version: None, params: Vec::new() };
        let inputs = reads.iter().map(|id| (wanted(id), None).into()).collect();
        Ok(Plan { inputs, files: Some(tiles.len() as u64 + 1) })
    }

    /// Each tile where the source has a pixel: made once, then uploaded with the batch of its tile.
    /// `state/tiles/` keeps the pixel digest of a made tile, or `.none` for a tile without a pixel,
    /// so a resume makes neither again.
    fn prepare(&self, job: &mut Job) -> Result<(), String> {
        let key = key(job.source).to_string();
        let marks = job.state.join("tiles");
        fs::create_dir_all(&marks).map_err(|e| format!("{}: {e}", marks.display()))?;
        let mut batch = Vec::new();
        for (x, y) in tiles(job.source, job.scope.as_ref())? {
            let (name, path) = (format!("{ZOOM}-{x}-{y}"), format!("{ZOOM}/{x}/{y}.tif"));
            let (digest, none) = (format!("{name}.sha256"), format!("{name}.none"));
            if job.done(&path) || marks.join(&none).exists() {
                continue;
            }
            if !(marks.join(&digest).exists() && job.work.join(&path).exists()) {
                let mut command = (self.tool)(job.root)?;
                command.args(["fetch", &key, "--tile", &name, "--out"]).arg(&job.work);
                job.program(&mut command)?;
                // The program writes the tile and then its digest, or `.none`.
                for mark in [&digest, &none].into_iter().filter(|mark| job.work.join(mark).exists()) {
                    fs::rename(job.work.join(mark), marks.join(mark)).map_err(|e| format!("{mark}: {e}"))?;
                }
                if marks.join(&none).exists() {
                    continue;
                }
                if !marks.join(&digest).exists() {
                    return Err(format!(
                        "`forest_reference.py fetch {key} --tile {name}` wrote neither a tile nor `.none`"
                    ));
                }
            }
            batch.push(path);
            if batch.len() == BATCH {
                job.finish(&std::mem::take(&mut batch))?;
            }
        }
        job.finish(&batch)?;
        // The index names the tiles of the version, each with the pixel digest of its mark.
        let (mut digests, mut tiles) = (BTreeMap::new(), BTreeMap::new());
        for path in job.files().map(|file| file.path.clone()).filter(|path| path.starts_with("12/")) {
            let tile = path["12/".len()..].trim_end_matches(".tif").to_string();
            let mark = marks.join(format!("{ZOOM}-{}.sha256", tile.replace('/', "-")));
            let digest = fs::read_to_string(&mark).map_err(|e| format!("{}: {e}", mark.display()))?;
            tiles.insert(tile, digest.trim().to_string());
            digests.insert(path, digest.trim().to_string());
        }
        let listed = job.state.join("digests.json");
        obc_data::store::write_atomic(&listed, &serde_json::to_vec(&tiles).expect("digests serialize"))?;
        let mut command = (self.tool)(job.root)?;
        command.args(["index-tiles", &key, "--fetched", &job.version, "--digests"]).arg(&listed);
        job.program(command.arg("--out").arg(job.work.join("index.json")))?;
        // The services have no versions of their own: the day of the prepare names what it read.
        let version = job.version.clone();
        for source in reads(&key)? {
            job.input(source, &version, digests.clone())?;
        }
        job.finish(&["index.json".into()])
    }
}

fn key(source: &Source) -> &str {
    &source.id["forest-reference/".len()..]
}

/// The zoom 12 tiles whose inside meets the `extent` of the source and, with a scope, the box of
/// its region. A scope of Geofabrik areas refuses: the tiles of a polygon are not needed yet.
fn tiles(source: &Source, scope: Option<&Scope>) -> Result<Vec<(u32, u32)>, String> {
    let [west, south, east, north] = source.extent.ok_or_else(|| format!("`{}` has no `extent`", source.id))?;
    let mut bbox = Bbox { west, south, east, north };
    if let Some(scope) = scope {
        let settings =
            Settings { region: scope.region.clone(), definitions: scope.definitions.clone(), ..Settings::default() };
        let region = settings.regions()?.bounds(&scope.region).ok_or_else(|| {
            format!("{}: a version of a region covers boxes; `{}` has another area", source.id, scope.region)
        })?;
        bbox = Bbox {
            west: bbox.west.max(region.west),
            south: bbox.south.max(region.south),
            east: bbox.east.min(region.east),
            north: bbox.north.min(region.north),
        };
        if bbox.west >= bbox.east || bbox.south >= bbox.north {
            return Ok(Vec::new());
        }
    }
    let (left, top) = mercator(bbox.west, bbox.north, ZOOM);
    let (right, bottom) = mercator(bbox.east, bbox.south, ZOOM);
    let last = (1u32 << ZOOM) - 1;
    let span = |from: f64, to: f64| (from.floor() as u32).min(last)..=((to.ceil() as u32).max(1) - 1).min(last);
    Ok(span(left, right).flat_map(|x| span(top, bottom).map(move |y| (x, y))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::tests::temp;
    use obc_data::engine::runs::Run;
    use obc_data::fetch::http::Http;
    use obc_data::prepared::Opening;
    use obc_data::sources::Registry;
    use obc_data::store::Store;
    use obc_data::target::Target;

    const SOURCES: &str = r#"
        [[source]]
        id = "forest-reference/ch"
        kind = "data"
        licence = "CC0-1.0"
        attribution = "Test"
        fetch = { kind = "prepared" }
        extent = [8.0, 46.5, 8.2, 46.6]
        version = "date"
        redistribute = true
    "#;

    /// The tool as a script: a tile with a file `none-<tile>` has no pixel, and every other tile
    /// gets its name as bytes and as digest.
    const TOOL: &str = r#"echo "$1 $4" >> calls
case "$1" in
fetch)
    if [ -e "none-$4" ]; then touch "$6/$4.none"; exit 0; fi
    t="${4#12-}"; mkdir -p "$6/12/${t%-*}" && echo "$4" > "$6/12/${t%-*}/${t#*-}.tif" && echo "pixels of $4" > "$6/$4.sha256" ;;
index-tiles)
    cp "$6" "$8" ;;
esac
"#;

    fn script(_: &Path) -> Result<Command, String> {
        let mut command = Command::new("sh");
        command.arg("tool.sh");
        Ok(command)
    }

    #[test]
    fn the_version_holds_each_tile_of_the_extent_that_has_a_pixel_with_its_pixel_digest() {
        let temp = temp("forest-reference");
        let root = temp.0.join("repository");
        fs::create_dir_all(root.join("data")).unwrap();
        fs::write(root.join("data/sources.toml"), SOURCES).unwrap();
        fs::write(root.join("tool.sh"), TOOL).unwrap();
        let (store, target) = (Store::at(temp.0.join("store")), Target::Dir(temp.0.join("target")));
        let sources = Registry::load(&root).unwrap().sources;
        let id = "forest-reference/ch";
        let tiles = tiles(sources.iter().find(|source| source.id == id).unwrap(), None).unwrap();
        // 8.0–8.2 °E and 46.5–46.6 °N meet three columns and two rows of zoom 12 tiles.
        assert_eq!(tiles.len(), 6, "{tiles:?}");
        let names: Vec<String> = tiles.iter().map(|(x, y)| format!("12-{x}-{y}")).collect();
        fs::write(root.join(format!("none-{}", names[0])), "").unwrap();
        let (producer, http) = (ForestReference { tool: script }, Http::new());
        let mut run = Run::create(&store, ["source", "prepare", id]).unwrap();
        let opening = Opening { root: &root, store: &store, http: &http, sources: &sources, target: ("t", &target) };
        let mut job = Job::open(opening, (id, "2026-10-20"), None, &[], &mut run).unwrap();
        producer.prepare(&mut job).unwrap();
        let Target::Dir(dir) = &target else { unreachable!() };
        let version = dir.join("sources/forest-reference/ch/2026-10-20");
        let index: BTreeMap<String, String> =
            serde_json::from_slice(&fs::read(version.join("index.json")).unwrap()).unwrap();
        let expected: BTreeMap<String, String> =
            names[1..].iter().map(|name| (name[3..].replace('-', "/"), format!("pixels of {name}"))).collect();
        assert_eq!(index, expected, "the tiles with a pixel, each with its pixel digest");
        for name in &names[1..] {
            let tile = version.join(format!("{}.tif", name.replacen('-', "/", 2)));
            assert_eq!(fs::read_to_string(tile).unwrap(), format!("{name}\n"));
        }
        assert!(!version.join(format!("{}.tif", names[0].replacen('-', "/", 2))).exists());
    }
}
