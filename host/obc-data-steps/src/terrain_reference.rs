//! The producer of `terrain-reference/<key>`: the national terrain model `dtm-<key>` as tiles of
//! the reference archive (`host/obc-dem/reference/README.md`), `16/<ti>/<tj>.tif` of the maximum
//! and `mean/16/<ti>/<tj>.tif` of the mean, and the `index.json` of each pool. The adapter of the
//! model, `ingest.py fetch`, pools one archive tile at a time.

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
use crate::prepared_tiles::{self, Tile};

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
        2
    }

    fn revision(&self) -> u32 {
        1
    }

    /// The model, and the polygons of its areas. The two pools of the tiles of its extent, in the box
    /// of a scope of boxes, and their two `index.json` are the files at most.
    fn plan(
        &self,
        _: &obc_data::prepared::Planning,
        source: &Source,
        scope: Option<&Scope>,
        _: &[String],
    ) -> Result<Plan, String> {
        let mut tiles = extent_tiles(source)?;
        if let Some(bbox) = scope.map(regions).transpose()?.and_then(|(parts, id)| parts.bounds(&id)) {
            let inside = box_tiles(&bbox)?;
            tiles.retain(|tile| inside.contains(tile));
        }
        let model = Wanted { source: model(source), version: None, params: Vec::new() };
        let polygons = source.areas.iter().map(|area| polygon(area, None));
        let inputs = std::iter::once(model).chain(polygons).map(|wanted| (wanted, None).into()).collect();
        Ok(Plan { inputs, files: Some(2 * tiles.len() as u64 + 2) })
    }

    /// Each tile that the model has a height in: fetched once, then its two pools uploaded with the
    /// batch of the tile. `state/tiles/` keeps the pixel digests of a fetched tile, or `.none` for a
    /// tile without a height, so a resume fetches neither again.
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
        if let Some(scope) = job.scope.clone() {
            let inside = scope_tiles(job, &scope)?;
            tiles.retain(|tile| inside.contains(tile));
        }
        // The program writes the tiles and then their digests, the maximum last, or `.none`.
        let tiles: Vec<Tile> = tiles
            .into_iter()
            .map(|tile| {
                let (name, path) =
                    (format!("{:04}-{:04}", tile.i, tile.j), format!("16/{:04}/{:04}.tif", tile.i, tile.j));
                let files =
                    vec![(format!("mean/{path}"), format!("{name}.mean.sha256")), (path, format!("{name}.sha256"))];
                Tile { name, files }
            })
            .collect();
        let digests = prepared_tiles::make(job, &tiles, |job, name, progress| {
            let mut command = (self.ingest)(job.root)?;
            // A folder per tile: `fetch` removes its `--work` at the end, and two tiles run at once.
            command.args(["fetch", key, "--tile", name, "--work"]).arg(job.state.join("raw").join(name));
            command.arg("--out").arg(&job.work).args(["--progress", progress]);
            Ok(command)
        })?;
        // Each index names the tiles of its pool, each with the pixel digest of its mark.
        for (pool, prefix) in [("max", ""), ("mean", "mean/")] {
            let tiles: BTreeMap<&str, &String> = digests
                .iter()
                .filter_map(|(path, digest)| Some((path.strip_prefix(prefix)?.strip_prefix("16/")?, digest)))
                .map(|(tile, digest)| (tile.trim_end_matches(".tif"), digest))
                .collect();
            let listed = job.state.join(format!("{pool}-digests.json"));
            obc_data::store::write_atomic(&listed, &serde_json::to_vec(&tiles).expect("digests serialize"))?;
            let mut command = (self.ingest)(job.root)?;
            command.args(["index-tiles", key, "--pool", pool, "--fetched", &job.version, "--digests"]).arg(&listed);
            job.program(command.arg("--out").arg(job.work.join(format!("{prefix}index.json"))))?;
        }
        // The model has no versions of its own: the day of the prepare names what it read.
        let version = job.version.clone();
        job.input(&model(source), &version, digests)?;
        job.finish(&["index.json".into(), "mean/index.json".into()])
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

/// The archive tiles of the region of `scope`: the box or the polygons of the areas of each part.
fn scope_tiles(job: &mut Job, scope: &Scope) -> Result<BTreeSet<CellId>, String> {
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
            Area::Union { .. } => {}
        }
    }
    Ok(tiles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::tests::{temp, Temp};
    use obc_data::engine::runs::Run;
    use obc_data::fetch::http::Http;
    use obc_data::prepared::Opening;
    use obc_data::sources::Registry;
    use obc_data::store::Store;
    use obc_data::target::Target;
    use std::path::PathBuf;

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

    /// `ingest.py` as a script that logs each call to `calls`, with the progress of a fetch, and
    /// the `--work` of a fetch to `works` once it holds a raw file there. A tile with a file
    /// `none-<tile>` has no height, and one with `fail-<tile>` fails. One with `wait-<tile>` waits
    /// until `works` has as many lines as that file says, and one with `hold-<tile>` runs until a
    /// signal stops it. `index-tiles` writes the digests that it gets unless `fail-index` exists.
    const INGEST: &str = r#"case "$1" in
fetch)
    echo "fetch $4 ${10}" >> calls
    # As `ingest.py fetch` does, it removes its `--work` when it ends, also on SIGTERM.
    trap 'rm -rf "$6"' EXIT && trap 'exit 143' TERM
    mkdir -p "$6" && touch "$6/raw.tif" && echo "$6" >> works
    if [ -e "wait-$4" ]; then
        n=0; until [ "$(wc -l < works)" -ge "$(cat "wait-$4")" ]; do n=$((n + 1)); [ $n -gt 1000 ] && exit 2; sleep 0.01; done
    fi
    if [ -e "hold-$4" ]; then while :; do sleep 0.01; done; fi
    [ -e "fail-$4" ] && exit 1
    if [ -e "none-$4" ]; then touch "$8/$4.none"; exit 0; fi
    for pool in "" mean/; do mkdir -p "$8/${pool}16/${4%-*}" && echo "$pool$4" > "$8/${pool}16/${4%-*}/${4#*-}.tif"; done
    echo "mean of $4" > "$8/$4.mean.sha256" && echo "pixels of $4" > "$8/$4.sha256" ;;
index-tiles)
    echo "$1 $4" >> calls
    [ -e fail-index ] && exit 1
    cp "$8" "${10}" ;;
esac
"#;

    fn script(_: &Path) -> Result<Command, String> {
        let mut command = Command::new("sh");
        command.arg("ingest.sh");
        Ok(command)
    }

    const ID: &str = "terrain-reference/xx";

    /// A repository with [`INGEST`], a store and a target for `terrain-reference/xx@2026-10-01`.
    struct Fixture {
        temp: Temp,
        sources: Vec<obc_data::sources::Source>,
        store: Store,
        target: Target,
        /// The tiles of the extent, in the order of the prepare.
        tiles: Vec<String>,
    }

    impl Fixture {
        fn new(name: &str) -> Fixture {
            let temp = temp(name);
            let root = temp.0.join("repository");
            fs::create_dir_all(root.join("data")).unwrap();
            fs::write(root.join("data/sources.toml"), SOURCES).unwrap();
            fs::write(root.join("ingest.sh"), INGEST).unwrap();
            let sources = Registry::load(&root).unwrap().sources;
            let tiles = extent_tiles(sources.iter().find(|source| source.id == ID).unwrap()).unwrap();
            let tiles: Vec<String> = tiles.iter().map(|tile| format!("{:04}-{:04}", tile.i, tile.j)).collect();
            assert!(tiles.len() > 3, "{tiles:?}");
            let (store, target) = (Store::at(temp.0.join("store")), Target::Dir(temp.0.join("target")));
            Fixture { temp, sources, store, target, tiles }
        }

        fn root(&self) -> PathBuf {
            self.temp.0.join("repository")
        }

        fn state(&self) -> PathBuf {
            self.store.root().join("prepared/t/terrain-reference/xx/2026-10-01/state")
        }

        /// Write the file `<what>-<tile k>` that changes what [`INGEST`] does.
        fn mark(&self, what: &str, k: usize, text: &str) {
            fs::write(self.root().join(format!("{what}-{}", self.tiles[k])), text).unwrap();
        }

        /// A run of the prepare, and the calls of the script, sorted: two fetches run at once.
        fn prepare(&self) -> (Result<(), String>, Vec<String>) {
            let root = self.root();
            let _ = fs::remove_file(root.join("calls"));
            let (producer, http) = (TerrainReference { ingest: script }, Http::new());
            let mut run = Run::create(&self.store, ["source", "prepare", ID]).unwrap();
            let opening = Opening {
                root: &root,
                store: &self.store,
                http: &http,
                sources: &self.sources,
                target: ("t", &self.target),
            };
            let done = Job::open(opening, (ID, "2026-10-01"), None, &[], &mut run)
                .and_then(|mut job| producer.prepare(&mut job));
            let mut calls: Vec<String> =
                fs::read_to_string(root.join("calls")).unwrap_or_default().lines().map(String::from).collect();
            calls.sort();
            (done, calls)
        }

        /// The calls of the fetches of tiles `k`, each with its progress among all tiles, sorted.
        fn fetches(&self, k: impl Iterator<Item = usize>) -> Vec<String> {
            let count = self.tiles.len();
            let mut calls: Vec<String> = k.map(|k| format!("fetch {} {}/{count}", self.tiles[k], k + 1)).collect();
            calls.sort();
            calls
        }
    }

    #[test]
    fn a_resume_fetches_no_tile_that_it_has_and_the_index_holds_the_digest_of_each_tile() {
        let fixture = Fixture::new("terrain-reference");
        let tiles = &fixture.tiles;
        let index = || vec!["index-tiles max".to_string(), "index-tiles mean".to_string()];

        fixture.mark("none", 0, "");
        fixture.mark("fail", 2, "");
        let (done, calls) = fixture.prepare();
        assert!(done.is_err());
        // The failure stops the fetch that runs beside it, which can be the next one.
        assert!(calls == fixture.fetches(0..3) || calls == fixture.fetches(0..4), "{calls:?}");

        // The tile in `work/` is uploaded with its digest, and no tile without a height is fetched.
        // The progress counts the tiles that the version has.
        fs::remove_file(fixture.root().join(format!("fail-{}", tiles[2]))).unwrap();
        fs::write(fixture.root().join("fail-index"), "").unwrap();
        let (done, calls) = fixture.prepare();
        assert!(done.is_err());
        assert_eq!(calls, [fixture.fetches(2..tiles.len()), index()[..1].to_vec()].concat());

        fs::remove_file(fixture.root().join("fail-index")).unwrap();
        // A mark of a tile that is not in the version.
        fs::write(fixture.state().join("tiles/9999-9999.sha256"), "pixels of 9999-9999\n").unwrap();
        let (done, calls) = fixture.prepare();
        done.unwrap();
        assert_eq!(calls, index(), "a resume fetches no tile that the target has");
        let Target::Dir(dir) = &fixture.target else { unreachable!() };
        let version = dir.join("sources/terrain-reference/xx/2026-10-01");
        for (prefix, pixels) in [("", "pixels of"), ("mean/", "mean of")] {
            let digests: BTreeMap<String, String> =
                tiles[1..].iter().map(|tile| (tile.replace('-', "/"), format!("{pixels} {tile}"))).collect();
            let index: BTreeMap<String, String> =
                serde_json::from_slice(&fs::read(version.join(format!("{prefix}index.json"))).unwrap()).unwrap();
            assert_eq!(index, digests, "the tiles of each pool, each with its pixel digest");
            for tile in &tiles[1..] {
                let (ti, tj) = tile.split_once('-').unwrap();
                let file = version.join(format!("{prefix}16/{ti}/{tj}.tif"));
                assert_eq!(fs::read_to_string(file).unwrap(), format!("{prefix}{tile}\n"));
            }
            assert!(!version.join(format!("{prefix}16/{}.tif", tiles[0].replace('-', "/"))).exists());
        }
    }

    #[test]
    fn two_fetches_at_once_have_a_raw_folder_each_and_a_failure_stops_the_other_which_removes_its_folder() {
        let fixture = Fixture::new("terrain-reference-raw");
        // Tile 0 ends only once tile 1 runs beside it. Tile 2 fails once tile 3 runs beside it, and
        // tile 3 runs until the failure stops it.
        fixture.mark("wait", 0, "2");
        fixture.mark("wait", 2, "4");
        fixture.mark("fail", 2, "");
        fixture.mark("hold", 3, "");
        let (done, calls) = fixture.prepare();
        assert!(done.is_err());
        assert_eq!(calls, fixture.fetches(0..4));
        let mut works: Vec<String> =
            fs::read_to_string(fixture.root().join("works")).unwrap().lines().map(String::from).collect();
        works.sort();
        let raw = fixture.state().join("raw");
        let mut folders: Vec<String> =
            fixture.tiles[..4].iter().map(|tile| raw.join(tile).display().to_string()).collect();
        folders.sort();
        assert_eq!(works, folders);
        assert_eq!(fs::read_dir(&raw).unwrap().count(), 0, "each fetch removed its folder before the end");
    }
}
