//! The producer of `terrain-reference/<key>`: the national terrain model `dtm-<key>` as tiles of
//! the reference archive (`host/obc-dem/reference/README.md`), `16/<ti>/<tj>.tif`, and their
//! `index.json`. The adapter of the model, `ingest.py fetch`, pools one archive tile at a time.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;

use obc_bake::coverage::Coverage;
use obc_data::prepared::{Job, Plan, Prepared, Scope};
use obc_data::product::Wanted;
use obc_data::regions::{Area, Bbox};
use obc_data::settings::Settings;
use obc_data::sources::Source;
use obc_dem::reference::TILE_LOG2;
use obc_map_core::grid::CellId;

use crate::device::{box_poly, POLY};

/// The tiles of one upload.
const BATCH: usize = 64;

pub struct TerrainReference {
    /// `ingest.py` of the reference archive, as a command in the repository root.
    pub ingest: fn(&Path) -> Result<Command, String>,
}

pub const TERRAIN_REFERENCE: TerrainReference = TerrainReference { ingest };

fn ingest(root: &Path) -> Result<Command, String> {
    let mut command = Command::new("uv");
    command.args(["run", "--locked", "--offline", "--no-default-groups", "--no-python-downloads"]);
    command.args(["--group", "terrain-reference", "python", "host/obc-dem/reference/ingest.py"]);
    command.env("UV_PYTHON", obc_data::engine::python_executable(root)?).env("UV_NO_SYNC", "0");
    Ok(command)
}

impl Prepared for TerrainReference {
    fn name(&self) -> &'static str {
        "terrain-reference"
    }

    fn format(&self) -> u32 {
        1
    }

    fn revision(&self) -> u32 {
        1
    }

    /// The model, and the polygons of its areas. The tiles of its extent, in the box of a scope of
    /// boxes, and `index.json` are the files at most.
    fn plan(&self, source: &Source, scope: Option<&Scope>) -> Result<Plan, String> {
        let mut tiles = extent_tiles(source)?;
        if let Some(bbox) = scope.map(regions).transpose()?.and_then(|(parts, id)| parts.bounds(&id)) {
            let inside = box_tiles(&bbox)?;
            tiles.retain(|tile| inside.contains(tile));
        }
        let model = Wanted { source: model(source), version: None, params: Vec::new() };
        let polygons = source.areas.iter().map(|area| polygon(area, None));
        let inputs = std::iter::once(model).chain(polygons).map(|wanted| (wanted, None)).collect();
        Ok(Plan { inputs, files: Some(tiles.len() as u64 + 1) })
    }

    /// Each tile that the model has a height in: fetched once, then uploaded with the batch of
    /// its tile. `state/tiles/` keeps the pixel digest of a fetched tile, or `.none` for a tile
    /// without a height, so a resume fetches neither again.
    fn prepare(&self, job: &mut Job) -> Result<(), String> {
        let source = job.source;
        let key = &source.id["terrain-reference/".len()..];
        let mut tiles = extent_tiles(source)?;
        if !source.areas.is_empty() {
            let mut inside = BTreeSet::new();
            for area in &source.areas {
                inside.extend(area_tiles(job, area)?);
            }
            tiles.retain(|tile| inside.contains(tile));
        }
        if let Some(inside) = job.scope.clone().map(|scope| scope_tiles(job, &scope)).transpose()?.flatten() {
            tiles.retain(|tile| inside.contains(tile));
        }
        let marks = job.state.join("tiles");
        fs::create_dir_all(&marks).map_err(|e| format!("{}: {e}", marks.display()))?;
        let mut batch = Vec::new();
        for tile in tiles {
            let (name, path) = (format!("{:04}-{:04}", tile.i, tile.j), format!("16/{:04}/{:04}.tif", tile.i, tile.j));
            let (digest, none) = (format!("{name}.sha256"), format!("{name}.none"));
            if job.done(&path) || marks.join(&none).exists() {
                continue;
            }
            if !(marks.join(&digest).exists() && job.work.join(&path).exists()) {
                let mut command = (self.ingest)(job.root)?;
                command.args(["fetch", key, "--tile", &name, "--work"]).arg(job.state.join("raw"));
                job.program(command.arg("--out").arg(&job.work))?;
                // The program writes the tile and then its digest, or `.none`.
                for mark in [&digest, &none].into_iter().filter(|mark| job.work.join(mark).exists()) {
                    fs::rename(job.work.join(mark), marks.join(mark)).map_err(|e| format!("{mark}: {e}"))?;
                }
                if marks.join(&none).exists() {
                    continue;
                }
                if !marks.join(&digest).exists() {
                    return Err(format!("`ingest.py fetch {key} --tile {name}` wrote neither a tile nor `.none`"));
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
        for path in job.files().map(|file| file.path.clone()).filter(|path| path.starts_with("16/")) {
            let tile = path["16/".len()..].trim_end_matches(".tif").to_string();
            let mark = marks.join(format!("{}.sha256", tile.replace('/', "-")));
            let digest = fs::read_to_string(&mark).map_err(|e| format!("{}: {e}", mark.display()))?;
            tiles.insert(tile, digest.trim().to_string());
            digests.insert(path, digest.trim().to_string());
        }
        let listed = job.state.join("digests.json");
        obc_data::store::write_atomic(&listed, &serde_json::to_vec(&tiles).expect("digests serialize"))?;
        let mut command = (self.ingest)(job.root)?;
        command.args(["index-tiles", key, "--fetched", &job.version, "--digests"]).arg(&listed);
        job.program(command.arg("--out").arg(job.work.join("index.json")))?;
        // The model has no versions of its own: the day of the prepare names what it read.
        let version = job.version.clone();
        job.input(&model(source), &version, digests)?;
        job.finish(&["index.json".into()])
    }
}

/// `dtm-<key>`, the national model of `terrain-reference/<key>`.
fn model(source: &Source) -> String {
    format!("dtm-{}", &source.id["terrain-reference/".len()..])
}

fn polygon(area: &str, version: Option<String>) -> Wanted {
    Wanted { source: POLY.into(), version, params: vec![("area".into(), area.into())] }
}

/// The archive tiles that `bbox` meets.
fn box_tiles(bbox: &Bbox) -> Result<BTreeSet<CellId>, String> {
    Ok(Coverage::parse_poly(&box_poly(bbox))?.cells(TILE_LOG2.into()))
}

fn extent_tiles(source: &Source) -> Result<BTreeSet<CellId>, String> {
    let [west, south, east, north] = source.extent.ok_or_else(|| format!("`{}` has no `extent`", source.id))?;
    box_tiles(&Bbox { west, south, east, north })
}

/// The archive tiles that the Geofabrik polygon of `area` meets. The first run of a version keeps
/// the version of each polygon in `state/polygons.json`, so its tiles stay those of the first run.
fn area_tiles(job: &mut Job, area: &str) -> Result<BTreeSet<CellId>, String> {
    let pins = job.state.join("polygons.json");
    let mut versions: BTreeMap<String, String> = match fs::read(&pins) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", pins.display()))?,
        Err(_) => BTreeMap::new(),
    };
    let pinned = versions.get(area).cloned();
    let (version, files) = job.fetch(&polygon(area, pinned.clone())).map_err(|e| match pinned {
        Some(_) => format!(
            "{e}. Fix: the polygon version of the first run is gone: delete {}; the next run starts a new version",
            job.state.parent().unwrap_or(&job.state).display()
        ),
        None => e,
    })?;
    if versions.insert(area.into(), version.clone()).as_ref() != Some(&version) {
        let bytes = serde_json::to_vec(&versions).expect("versions serialize");
        obc_data::store::write_atomic(&pins, &bytes)?;
    }
    let [path] = files.values().collect::<Vec<_>>()[..] else {
        return Err(format!("a fetch of {POLY} of `{area}` gives {} files, not one", files.len()));
    };
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Coverage::parse_poly(&text)?.cells(TILE_LOG2.into()))
}

/// The regions of `scope`, and the id of its region.
fn regions(scope: &Scope) -> Result<(obc_data::regions::Regions, String), String> {
    let settings =
        Settings { region: scope.region.clone(), definitions: scope.definitions.clone(), ..Settings::default() };
    Ok((settings.regions()?, scope.region.clone()))
}

/// The archive tiles of the region of `scope`: the box or the polygons of the areas of each part;
/// `None` for the planet, which holds every tile.
fn scope_tiles(job: &mut Job, scope: &Scope) -> Result<Option<BTreeSet<CellId>>, String> {
    let (regions, id) = regions(scope)?;
    let mut tiles = BTreeSet::new();
    for part in regions.leaves(&id)? {
        match &regions.get(part).expect("a part is a region").area {
            Area::Box { bbox } => tiles.extend(box_tiles(bbox)?),
            Area::Geofabrik { areas } => {
                for area in areas {
                    tiles.extend(area_tiles(job, area)?);
                }
            }
            Area::Planet => return Ok(None),
            Area::Union { .. } => {}
        }
    }
    Ok(Some(tiles))
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
        id = "dtm-xx"
        kind = "data"
        licence = "CC0-1.0"
        attribution = "Test"
        fetch = { kind = "dtm", url = "https://example.org/xx" }
        extent = [8.0, 46.5, 8.2, 46.6]
        version = "date"
        redistribute = true
    "#;

    /// `ingest.py` as a script that logs each call to `calls`: a tile with a file `none-<tile>`
    /// has no height, one with `fail-<tile>` fails, and `index-tiles` writes the digests that it
    /// gets unless `fail-index` exists.
    const INGEST: &str = r#"echo "$1 $4" >> calls
case "$1" in
fetch)
    [ -e "fail-$4" ] && exit 1
    if [ -e "none-$4" ]; then touch "$8/$4.none"; exit 0; fi
    mkdir -p "$8/16/${4%-*}" && echo "$4" > "$8/16/${4%-*}/${4#*-}.tif" && echo "pixels of $4" > "$8/$4.sha256" ;;
index-tiles)
    [ -e fail-index ] && exit 1
    cp "$6" "$8" ;;
esac
"#;

    fn script(_: &Path) -> Result<Command, String> {
        let mut command = Command::new("sh");
        command.arg("ingest.sh");
        Ok(command)
    }

    #[test]
    fn a_resume_fetches_no_tile_that_it_has_and_the_index_holds_the_digest_of_each_tile() {
        let temp = temp("terrain-reference");
        let root = temp.0.join("repository");
        fs::create_dir_all(root.join("data")).unwrap();
        fs::write(root.join("data/sources.toml"), SOURCES).unwrap();
        fs::write(root.join("ingest.sh"), INGEST).unwrap();
        let (store, target) = (Store::at(temp.0.join("store")), Target::Dir(temp.0.join("target")));
        let sources = Registry::load(&root).unwrap().sources;
        let id = "terrain-reference/xx";
        let tiles: Vec<String> = extent_tiles(sources.iter().find(|source| source.id == id).unwrap())
            .unwrap()
            .iter()
            .map(|tile| format!("{:04}-{:04}", tile.i, tile.j))
            .collect();
        assert!(tiles.len() > 3, "{tiles:?}");
        let (producer, http) = (TerrainReference { ingest: script }, Http::new());
        let prepare = || {
            let _ = fs::remove_file(root.join("calls"));
            let mut run = Run::create(&store, ["source", "prepare", id]).unwrap();
            let opening =
                Opening { root: &root, store: &store, http: &http, sources: &sources, target: ("t", &target) };
            let done =
                Job::open(opening, (id, "2026-10-01"), None, &mut run).and_then(|mut job| producer.prepare(&mut job));
            let calls = fs::read_to_string(root.join("calls")).unwrap_or_default();
            (done, calls.lines().map(String::from).collect::<Vec<_>>())
        };
        let fetches = |tiles: &[String]| tiles.iter().map(|tile| format!("fetch {tile}")).collect::<Vec<_>>();
        let index = || vec!["index-tiles 2026-10-01".to_string()];

        fs::write(root.join(format!("none-{}", tiles[0])), "").unwrap();
        fs::write(root.join(format!("fail-{}", tiles[2])), "").unwrap();
        let (done, calls) = prepare();
        assert!(done.is_err());
        assert_eq!(calls, fetches(&tiles[..3]));

        // The tile in `work/` is uploaded with its digest, and no tile without a height is fetched.
        fs::remove_file(root.join(format!("fail-{}", tiles[2]))).unwrap();
        fs::write(root.join("fail-index"), "").unwrap();
        let (done, calls) = prepare();
        assert!(done.is_err());
        assert_eq!(calls, [fetches(&tiles[2..]), index()].concat());

        fs::remove_file(root.join("fail-index")).unwrap();
        // A mark of a tile that is not in the version.
        let marks = store.root().join("prepared/t/terrain-reference/xx/2026-10-01/state/tiles");
        fs::write(marks.join("9999-9999.sha256"), "pixels of 9999-9999\n").unwrap();
        let (done, calls) = prepare();
        done.unwrap();
        assert_eq!(calls, index(), "a resume fetches no tile that the target has");
        let Target::Dir(dir) = &target else { unreachable!() };
        let version = dir.join("sources/terrain-reference/xx/2026-10-01");
        let digests: BTreeMap<String, String> =
            tiles[1..].iter().map(|tile| (tile.replace('-', "/"), format!("pixels of {tile}"))).collect();
        let index: BTreeMap<String, String> =
            serde_json::from_slice(&fs::read(version.join("index.json")).unwrap()).unwrap();
        assert_eq!(index, digests, "the tiles of the version, each with its pixel digest");
        for tile in &tiles[1..] {
            let (ti, tj) = tile.split_once('-').unwrap();
            assert_eq!(fs::read_to_string(version.join(format!("16/{ti}/{tj}.tif"))).unwrap(), format!("{tile}\n"));
        }
        assert!(!version.join(format!("16/{}", tiles[0].replace('-', "/"))).with_extension("tif").exists());
    }
}
