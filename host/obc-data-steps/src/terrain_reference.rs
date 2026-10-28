//! The producer of `terrain`: the national terrain models `dtm-<key>` as tiles of the reference
//! archive (`host/obc-dem/reference/README.md`), `16/<ti>/<tj>.tif` of the maximum and
//! `mean/16/<ti>/<tj>.tif` of the mean, and the `index.json` of each pool. A version extends the
//! version before: `ingest.py fetch` folds the models that meet its region into each tile of that
//! region, and the version keeps every other tile of the version before without an upload. Each
//! index states the GLO-30 bias of each model (`crate::terrain_bias`).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;

use obc_bake::coverage::Coverage;
use obc_data::prepared::{day, Job, Manifest, Plan, Prepared, Scope};
use obc_data::product::Wanted;
use obc_data::regions::{Area, Bbox};
use obc_data::settings::Settings;
use obc_data::sources::Source;
use obc_dem::reference::TILE_LOG2;
use obc_map_core::grid::CellId;

use crate::device::{box_poly, POLY};
use crate::prepared_tiles::{self, Tile};

/// The id of the prepared source.
pub const ID: &str = "terrain";

pub struct TerrainReference {
    /// `ingest.py` of the reference archive, as a command in the repository root.
    pub ingest: fn(&Path) -> Result<Command, String>,
    /// The GLO-30 squares that the bias of a model reads.
    pub(crate) glo30: crate::terrain_bias::Glo30,
}

pub const TERRAIN_REFERENCE: TerrainReference = TerrainReference { ingest, glo30: crate::terrain_bias::fetch };

fn ingest(root: &Path) -> Result<Command, String> {
    let mut command = Command::new("uv");
    command.args(["run", "--locked", "--offline", "--no-default-groups", "--no-python-downloads"]);
    command.args(["--group", "terrain-reference", "python", "host/obc-dem/reference/ingest.py"]);
    command.env("UV_PYTHON", obc_data::engine::python_executable(root)?).env("UV_NO_SYNC", "0");
    Ok(command)
}

impl Prepared for TerrainReference {
    fn name(&self) -> &'static str {
        ID
    }

    fn format(&self) -> u32 {
        3
    }

    fn revision(&self) -> u32 {
        2
    }

    fn regional(&self) -> bool {
        true
    }

    /// The models that meet the region, and the polygons of their areas. The two pools of the tiles
    /// of each model in the box of the region, and the two `index.json`, are the files at most.
    fn plan(
        &self,
        planning: &obc_data::prepared::Planning,
        _: &Source,
        scope: Option<&Scope>,
        _: &[String],
    ) -> Result<Plan, String> {
        let (boxes, countries) = area(scope)?;
        let held = boxes.as_deref().map(|boxes| boxes.iter().map(box_tiles).collect::<Result<Vec<_>, _>>());
        let held = held.transpose()?;
        let (mut inputs, mut tiles) = (Vec::new(), BTreeSet::new());
        for model in models(planning.sources, boxes.as_deref(), &countries) {
            let mut inside = extent_tiles(model)?;
            if let Some(held) = &held {
                inside.retain(|tile| held.iter().any(|tiles| tiles.contains(tile)));
            }
            tiles.extend(inside);
            inputs.push(Wanted { source: model.id.clone(), version: None, params: Vec::new() });
            inputs.extend(model.areas.iter().map(|area| polygon(area, None)));
        }
        let inputs = inputs.into_iter().map(|wanted| (wanted, None).into()).collect();
        Ok(Plan { inputs, files: Some(2 * tiles.len() as u64 + 2) })
    }

    /// Each tile of the region that a model has a height in: its models fetched once and folded
    /// into the tile of the version before, then its two pools uploaded with the batch of the tile.
    /// `state/tiles/` keeps the pixel digests and the sources of a made tile, or `.none` for a tile
    /// without a height, so a resume fetches neither again.
    fn prepare(&self, job: &mut Job) -> Result<(), String> {
        let previous = job.extend(self.format())?;
        let scope = match job.scope.clone() {
            Some(scope) => Some(scope_tiles(job, &scope)?),
            None => None,
        };
        let (boxes, countries) = area(job.scope.as_ref())?;
        // The models of each tile, in the order of `data/sources.toml`.
        let mut keys: BTreeMap<CellId, Vec<String>> = BTreeMap::new();
        for model in models(job.sources(), boxes.as_deref(), &countries) {
            let mut tiles = extent_tiles(model)?;
            if !model.areas.is_empty() {
                let mut inside = BTreeSet::new();
                for area in &model.areas {
                    inside.extend(area_tiles(job, area)?);
                }
                tiles.retain(|tile| inside.contains(tile));
            }
            if let Some(scope) = &scope {
                tiles.retain(|tile| scope.contains(tile));
            }
            for tile in tiles {
                keys.entry(tile).or_default().push(key(model).into());
            }
        }
        let before = previous.as_ref().map(|manifest| index(job, manifest, "")).transpose()?;
        let held = before.as_ref().map(|index| index.contributors.clone()).unwrap_or_default();
        let tiles: Vec<Tile> = keys
            .keys()
            .map(|tile| {
                let name = format!("{:04}-{:04}", tile.i, tile.j);
                let path = format!("16/{:04}/{:04}.tif", tile.i, tile.j);
                let files =
                    vec![(format!("mean/{path}"), format!("{name}.mean.sha256")), (path, format!("{name}.sha256"))];
                Tile { name: name.clone(), files, marks: vec![format!("{name}.keys"), format!("{name}.samples")] }
            })
            .collect();
        let models: BTreeMap<String, String> =
            keys.iter().map(|(tile, keys)| (format!("{:04}-{:04}", tile.i, tile.j), keys.join(","))).collect();
        let base = job.state.join("base");
        let digests = prepared_tiles::make(job, &tiles, |job, name| {
            let mut command = (self.ingest)(job.root)?;
            // A folder per tile: `fetch` removes its `--work` at the end, and tiles run at once.
            command.args(["fetch", &models[name], "--tile", name, "--work"]).arg(job.state.join("raw").join(name));
            command.arg("--out").arg(&job.work);
            let id = name.replace('-', "/");
            if let (Some(previous), Some(sources)) = (&previous, held.get(&id)) {
                for prefix in ["", "mean/"] {
                    let path = format!("{prefix}16/{id}.tif");
                    link(&job.earlier_file(previous, &path)?, &base.join(&path))?;
                }
                command.arg("--base").arg(&base).args(["--held", &sources.join(",")]);
            }
            Ok(command)
        })?;
        // A tile that this version did not make is the tile of the version before.
        if let Some(previous) = &previous {
            let kept: Vec<String> = previous
                .files
                .iter()
                .map(|file| file.path.clone())
                .filter(|path| path.ends_with(".tif") && !digests.contains_key(path))
                .collect();
            for path in kept {
                job.reuse(previous, &path)?;
            }
        }
        let marks = job.state.join("tiles");
        let mut sources: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for tile in tiles.iter().filter(|tile| digests.contains_key(&tile.files[1].0)) {
            let text = fs::read_to_string(marks.join(&tile.marks[0])).map_err(|e| format!("{}: {e}", tile.marks[0]))?;
            sources.insert(tile.name.replace('-', "/"), text.trim().split(',').map(String::from).collect());
        }
        let biases = crate::terrain_bias::measure(job, (previous.as_ref(), before.as_ref()), &sources, self.glo30)?;
        let biased = job.state.join("biases.json");
        obc_data::store::write_atomic(&biased, &serde_json::to_vec(&biases).expect("biases serialize"))?;
        let fetched: BTreeSet<&String> = keys.values().flatten().collect();
        let fetched: Vec<&str> = fetched.into_iter().map(String::as_str).collect();
        // A model has no versions of its own: the day of the prepare names what it read.
        let version = day(&job.version).to_string();
        // Each index names each tile of its pool, with its sources and the pixel digest of its mark.
        for (pool, prefix) in [("max", ""), ("mean", "mean/")] {
            let made: BTreeMap<String, &String> = digests
                .iter()
                .filter_map(|(path, digest)| Some((path.strip_prefix(prefix)?.strip_prefix("16/")?, digest)))
                .map(|(tile, digest)| (tile.trim_end_matches(".tif").to_string(), digest))
                .collect();
            let listed = job.state.join(format!("{pool}-digests.json"));
            obc_data::store::write_atomic(&listed, &serde_json::to_vec(&made).expect("digests serialize"))?;
            let held = job.state.join("keys.json");
            obc_data::store::write_atomic(&held, &serde_json::to_vec(&sources).expect("keys serialize"))?;
            let mut command = (self.ingest)(job.root)?;
            command.args(["index-terrain", "--pool", pool, "--fetched", &version, "--models", &fetched.join(",")]);
            command.arg("--digests").arg(&listed).arg("--keys").arg(&held).arg("--biases").arg(&biased);
            if let Some(previous) = &previous {
                command.arg("--base").arg(job.earlier_file(previous, &format!("{prefix}index.json"))?);
            }
            job.program(command.arg("--out").arg(job.work.join(format!("{prefix}index.json"))))?;
        }
        for model in &fetched {
            let read: BTreeMap<String, String> = digests
                .iter()
                .filter(|(path, _)| {
                    let tile = path.trim_start_matches("mean/").trim_start_matches("16/").trim_end_matches(".tif");
                    sources.get(tile).is_some_and(|keys| keys.iter().any(|key| key == model))
                })
                .map(|(path, digest)| (path.clone(), digest.clone()))
                .collect();
            job.input(&format!("dtm-{model}"), &version, read)?;
        }
        job.finish(&["index.json".into(), "mean/index.json".into()])
    }
}

/// The box of each leaf of the region of `scope` and its countries; no boxes for a version of
/// everything or a region with a Geofabrik leaf. A region that names no countries has those of its
/// leaves, and none when a leaf names none: that leaf reads every country. The planner reads the
/// same list.
pub(crate) fn area(scope: Option<&Scope>) -> Result<(Option<Vec<Bbox>>, Vec<String>), String> {
    let Some((regions, id)) = scope.map(regions).transpose()? else { return Ok((None, Vec::new())) };
    let own = regions.get(&id).map(|region| region.countries.clone()).unwrap_or_default();
    if !own.is_empty() {
        return Ok((regions.boxes(&id), own));
    }
    let mut countries = Vec::new();
    for leaf in regions.leaves(&id)? {
        let named = &regions.get(leaf).expect("a leaf is a region").countries;
        if named.is_empty() {
            countries.clear();
            break;
        }
        countries.extend(named.iter().cloned());
    }
    countries.sort();
    countries.dedup();
    Ok((regions.boxes(&id), countries))
}

/// Whether the `extent` of `source` meets one of `boxes`; always without boxes.
pub(crate) fn meets(source: &Source, boxes: Option<&[Bbox]>) -> bool {
    boxes.is_none_or(|boxes| boxes.iter().any(|b| source.meets([b.west, b.south, b.east, b.north])))
}

/// The national models of `sources` that may be given to others, that meet `boxes`, and that are
/// of one of `countries` when the region names some. The first part of the key is the ISO 3166-1
/// code of the country: `dtm-de-by` is of `DE`.
fn models<'a>(
    sources: &'a [Source],
    boxes: Option<&'a [Bbox]>,
    countries: &'a [String],
) -> impl Iterator<Item = &'a Source> {
    sources.iter().filter(move |source| {
        let country = source.id.trim_start_matches("dtm-").split('-').next().unwrap_or_default().to_uppercase();
        source.id.starts_with("dtm-")
            && source.redistribute
            && meets(source, boxes)
            && (countries.is_empty() || countries.contains(&country))
    })
}

/// The key of a national model: `dtm-ch` is `ch`.
fn key(model: &Source) -> &str {
    &model.id["dtm-".len()..]
}

/// The sources of each tile of the pool `prefix` of the version before, its best source and the
/// facts of each source.
#[derive(serde::Deserialize)]
pub(crate) struct Index {
    contributors: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub tiles: BTreeMap<String, String>,
    #[serde(default)]
    pub sources: BTreeMap<String, serde_json::Value>,
}

fn index(job: &Job, manifest: &Manifest, prefix: &str) -> Result<Index, String> {
    let path = job.earlier_file(manifest, &format!("{prefix}index.json"))?;
    serde_json::from_slice(&fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| format!("{}@{} {prefix}index.json: {e}", job.id, manifest.version))
}

/// A link of the store object `object` at `path`, for `ingest.py fetch --base`.
fn link(object: &Path, path: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    fs::create_dir_all(path.parent().expect("a tile has a folder")).map_err(|e| format!("{}: {e}", path.display()))?;
    fs::hard_link(object, path)
        .or_else(|_| fs::copy(object, path).map(|_| ()))
        .map_err(|e| format!("{}: {e}", path.display()))
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
    use crate::prepared_tiles::RUNNING;
    use obc_data::engine::runs::Run;
    use obc_data::fetch::http::Http;
    use obc_data::prepared::Manifest;
    use obc_data::prepared::Opening;
    use obc_data::sources::Registry;
    use obc_data::store::Store;
    use obc_data::target::Target;
    use obc_dem::fetch::TileId;
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
        keep = "prepared"
        redistribute = true

        [[source]]
        id = "terrain"
        kind = "data"
        licence = "CC0-1.0"
        attribution = "Test"
        fetch = { kind = "prepared" }
        version = "date"
        keep = "prepared"
        redistribute = true
    "#;

    /// `ingest.py` as a script that logs each call to `calls`, and the `--work` of a fetch to
    /// `works` once it holds a raw file there. A fetch over a tile of the version before logs
    /// `over` and the sources of that tile. A tile with a file
    /// `none-<tile>` has no height, and one with `fail-<tile>` fails. One with `wait-<tile>` waits
    /// until `works` has as many lines as that file says, and one with `hold-<tile>` runs until a
    /// signal stops it. `index-terrain` writes the digests that it gets unless `fail-index` exists.
    const INGEST: &str = r#"case "$1" in
fetch)
    echo "fetch $4 $2${10:+ over ${12}}" >> calls
    # As `ingest.py fetch` does, it removes its `--work` when it ends, also on SIGTERM.
    trap 'rm -rf "$6"' EXIT && trap 'exit 143' TERM
    mkdir -p "$6" && touch "$6/raw.tif" && echo "$6" >> works
    if [ -e "wait-$4" ]; then
        n=0; until [ "$(wc -l < works)" -ge "$(cat "wait-$4")" ]; do n=$((n + 1)); [ $n -gt 1000 ] && exit 2; sleep 0.01; done
    fi
    if [ -e "hold-$4" ]; then while :; do sleep 0.01; done; fi
    [ -e "fail-$4" ] && exit 1
    if [ -e "none-$4" ]; then touch "$8/$4.none"; exit 0; fi
    for pool in "" mean/; do mkdir -p "$8/${pool}16/${4%-*}" && echo "$pool$4 $2" > "$8/${pool}16/${4%-*}/${4#*-}.tif"; done
    echo "$2" > "$8/$4.keys"
    yes 15000 | head -n 256 > "$8/$4.samples"
    echo "mean of $4" > "$8/$4.mean.sha256" && echo "pixels of $4" > "$8/$4.sha256" ;;
index-terrain)
    over=""; [ $# -gt 15 ] && over=" over"
    echo "$1 $3$over" >> calls
    [ -e fail-index ] && exit 1
    eval "out=\${$#}"
    printf '{"contributors": %s, "sha256": %s}' "$(cat "${11}")" "$(cat "$9")" > "$out" ;;
esac
"#;

    fn script(_: &Path) -> Result<Command, String> {
        let mut command = Command::new("sh");
        command.arg("ingest.sh");
        Ok(command)
    }

    /// GLO-30 as all sea: it logs the squares that it is asked for to `squares`.
    fn sea(job: &mut Job, squares: &BTreeSet<TileId>) -> Result<BTreeMap<TileId, PathBuf>, String> {
        let names: Vec<String> = squares.iter().map(|tile| tile.stem()).collect();
        fs::write(job.root.join("squares"), names.join("\n")).map_err(|e| e.to_string())?;
        Ok(BTreeMap::new())
    }

    /// A repository with [`INGEST`], a store and a target for `terrain-reference/xx@2026-10-01`.
    struct Fixture {
        temp: Temp,
        sources: Vec<obc_data::sources::Source>,
        store: Store,
        target: Target,
        /// The tiles of the extent, in the order of the prepare.
        tiles: Vec<String>,
        cells: Vec<CellId>,
    }

    impl Fixture {
        fn new(name: &str) -> Fixture {
            let temp = temp(name);
            let root = temp.0.join("repository");
            fs::create_dir_all(root.join("data")).unwrap();
            fs::write(root.join("data/sources.toml"), SOURCES).unwrap();
            fs::write(root.join("ingest.sh"), INGEST).unwrap();
            let sources = Registry::load(&root).unwrap().sources;
            let tiles = extent_tiles(sources.iter().find(|source| source.id == "dtm-xx").unwrap()).unwrap();
            let cells: Vec<CellId> = tiles.into_iter().collect();
            let tiles: Vec<String> = cells.iter().map(|tile| format!("{:04}-{:04}", tile.i, tile.j)).collect();
            assert!(tiles.len() > 3, "{tiles:?}");
            let (store, target) = (Store::at(temp.0.join("store")), Target::Dir(temp.0.join("target")));
            Fixture { temp, sources, store, target, tiles, cells }
        }

        fn root(&self) -> PathBuf {
            self.temp.0.join("repository")
        }

        fn state(&self) -> PathBuf {
            self.store.root().join("prepared/t/terrain/2026-10-01/state")
        }

        /// Write the file `<what>-<tile k>` that changes what [`INGEST`] does.
        fn mark(&self, what: &str, k: usize, text: &str) {
            fs::write(self.root().join(format!("{what}-{}", self.tiles[k])), text).unwrap();
        }

        /// A run of the prepare, and the calls of the script, sorted: fetches run at once.
        fn prepare(&self) -> (Result<(), String>, Vec<String>) {
            let (done, calls, _, _) = self.version("2026-10-01", None);
            (done, calls)
        }

        /// A run of the prepare of `version` for `scope`: its end, the calls of the script, sorted,
        /// the reused files, complete on the target when the run ended well, and the status of the run.
        fn version(
            &self,
            version: &str,
            scope: Option<Scope>,
        ) -> (Result<(), String>, Vec<String>, Vec<String>, serde_json::Value) {
            let root = self.root();
            let _ = fs::remove_file(root.join("calls"));
            let _ = fs::remove_file(root.join("squares"));
            let (producer, http) = (TerrainReference { ingest: script, glo30: sea }, Http::new());
            let mut run = Run::create(&self.store, ["source", "prepare", ID]).unwrap();
            let opening = Opening {
                root: &root,
                store: &self.store,
                http: &http,
                sources: &self.sources,
                target: ("t", &self.target),
            };
            let mut reused = Vec::new();
            let done = Job::open(opening, (ID, version), scope, &[], &mut run).and_then(|mut job| {
                producer.prepare(&mut job)?;
                reused = job.reused().keys().cloned().collect();
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
                obc_data::prepared::complete(&self.target, &manifest).map(|_| ())
            });
            let mut calls: Vec<String> =
                fs::read_to_string(root.join("calls")).unwrap_or_default().lines().map(String::from).collect();
            calls.sort();
            run.flush().unwrap();
            let status = serde_json::from_slice(&fs::read(run.status_file()).unwrap()).unwrap();
            (done, calls, reused, status)
        }

        /// The calls of the fetches of tiles `k`, sorted.
        fn fetches(&self, k: impl Iterator<Item = usize>) -> Vec<String> {
            let mut calls: Vec<String> = k.map(|k| format!("fetch {} xx", self.tiles[k])).collect();
            calls.sort();
            calls
        }

        /// Whether `done` failed in the program of tile `k`, named with its progress.
        fn failed_in(&self, done: Result<(), String>, k: usize) -> bool {
            let label = format!("tile {}/{} {}: ", k + 1, self.tiles.len(), self.tiles[k]);
            done.is_err_and(|e| e.starts_with(&label))
        }
    }

    #[test]
    fn a_resume_fetches_no_tile_that_it_has_and_the_index_holds_the_digest_of_each_tile() {
        let fixture = Fixture::new("terrain-reference");
        let tiles = &fixture.tiles;
        let index = || vec!["index-terrain max".to_string(), "index-terrain mean".to_string()];

        let last = tiles.len() - 1;
        // Tile 2 fails once the two tiles after the first that run beside it have started, when it
        // is the oldest that runs, and the failure stops the tiles after it.
        let started = RUNNING + 2;
        assert!(tiles.len() > started, "{tiles:?}");
        fixture.mark("none", 0, "");
        fixture.mark("fail", 2, "");
        fixture.mark("wait", 2, &started.to_string());
        let (done, calls) = fixture.prepare();
        assert!(fixture.failed_in(done, 2));
        assert_eq!(calls, fixture.fetches(0..started));

        // No tile without a height is fetched, and the progress counts each tile of the version.
        for what in ["fail", "wait"] {
            fs::remove_file(fixture.root().join(format!("{what}-{}", tiles[2]))).unwrap();
        }
        fixture.mark("fail", last, "");
        let (done, calls) = fixture.prepare();
        assert!(fixture.failed_in(done, last));
        assert_eq!(calls, fixture.fetches(2..tiles.len()));

        // The tiles in `work/` are uploaded with their digests.
        fs::remove_file(fixture.root().join(format!("fail-{}", tiles[last]))).unwrap();
        fs::write(fixture.root().join("fail-index"), "").unwrap();
        let (done, calls) = fixture.prepare();
        assert!(done.is_err());
        assert_eq!(calls, [fixture.fetches(last..tiles.len()), index()[..1].to_vec()].concat());

        fs::remove_file(fixture.root().join("fail-index")).unwrap();
        // A mark of a tile that is not in the version.
        fs::write(fixture.state().join("tiles/9999-9999.sha256"), "pixels of 9999-9999\n").unwrap();
        let (done, calls) = fixture.prepare();
        done.unwrap();
        assert_eq!(calls, index(), "a resume fetches no tile that the target has");
        let Target::Dir(dir) = &fixture.target else { unreachable!() };
        let version = dir.join("sources/terrain/2026-10-01");
        for (prefix, pixels) in [("", "pixels of"), ("mean/", "mean of")] {
            let digests: BTreeMap<String, String> =
                tiles[1..].iter().map(|tile| (tile.replace('-', "/"), format!("{pixels} {tile}"))).collect();
            let index: serde_json::Value =
                serde_json::from_slice(&fs::read(version.join(format!("{prefix}index.json"))).unwrap()).unwrap();
            let index: BTreeMap<String, String> = serde_json::from_value(index["sha256"].clone()).unwrap();
            assert_eq!(index, digests, "the tiles of each pool, each with its pixel digest");
            for tile in &tiles[1..] {
                let (ti, tj) = tile.split_once('-').unwrap();
                let file = version.join(format!("{prefix}16/{ti}/{tj}.tif"));
                assert_eq!(fs::read_to_string(file).unwrap(), format!("{prefix}{tile} xx\n"));
            }
            assert!(!version.join(format!("{prefix}16/{}.tif", tiles[0].replace('-', "/"))).exists());
        }
    }

    #[test]
    fn fetches_at_once_have_a_raw_folder_each_and_a_failure_stops_the_others_which_remove_their_folders() {
        let fixture = Fixture::new("terrain-reference-raw");
        // Tile 0 ends only once tile 1 runs beside it. Tile 2 fails when it is the oldest that runs,
        // and tile 3 runs until the failure stops it.
        let started = RUNNING + 2;
        fixture.mark("wait", 0, "2");
        fixture.mark("wait", 2, &started.to_string());
        fixture.mark("fail", 2, "");
        fixture.mark("hold", 3, "");
        let (done, calls) = fixture.prepare();
        assert!(fixture.failed_in(done, 2));
        assert_eq!(calls, fixture.fetches(0..started));
        let mut works: Vec<String> =
            fs::read_to_string(fixture.root().join("works")).unwrap().lines().map(String::from).collect();
        works.sort();
        let raw = fixture.state().join("raw");
        let mut folders: Vec<String> =
            fixture.tiles[..started].iter().map(|tile| raw.join(tile).display().to_string()).collect();
        folders.sort();
        assert_eq!(works, folders);
        assert_eq!(fs::read_dir(&raw).unwrap().count(), 0, "each fetch removed its folder before the end");
    }

    #[test]
    fn a_version_folds_the_tiles_of_its_region_and_keeps_the_others_of_the_version_before() {
        let fixture = Fixture::new("terrain-extend");
        let (done, _, _, _) = fixture.version("2026-10-01", None);
        done.unwrap();
        // The model has no bias yet, so its samples ask GLO-30 for the squares under them.
        let squares = fs::read_to_string(fixture.root().join("squares")).unwrap();
        assert!(squares.lines().any(|square| square == "Copernicus_DSM_COG_10_N46_00_E008_00_DEM"), "{squares}");
        // A box inside the first tile.
        let (west, south, east, north) = fixture.cells[0].square();
        let inner = |a: i64, b: i64| (a + (b - a) / 4) as f64 / 1e6;
        let definition = format!(
            "name = \"Box\"\nkind = \"box\"\nbox = [{}, {}, {}, {}]\n",
            inner(west, east),
            inner(south, north),
            inner(east, west),
            inner(north, south)
        );
        let scope = Scope { region: "box".into(), definitions: [("box".to_string(), definition)].into() };
        let (done, calls, reused, status) = fixture.version("2026-10-02", Some(scope));
        done.unwrap();
        let first = &fixture.tiles[0];
        // The run shows the tile that it made as a step of `terrain`, with its time.
        assert_eq!(
            (&status["layers"][0]["layer"], &status["layers"][0]["done"], &status["layers"][0]["total"]),
            (&"terrain".into(), &1.into(), &1.into())
        );
        let tile = &status["steps"][0];
        assert_eq!(tile["step"], format!("terrain/{first}"));
        assert!(tile["wall_ms"].is_u64() && tile["bytes_out"].as_u64() > Some(0), "{tile}");
        let expected =
            [format!("fetch {first} xx over xx"), "index-terrain max over".into(), "index-terrain mean over".into()];
        assert_eq!(calls, expected, "only the tile of the region, over the tile before");
        let kept: Vec<String> = fixture.tiles[1..]
            .iter()
            .flat_map(|tile| {
                let path = format!("16/{}.tif", tile.replace('-', "/"));
                [format!("mean/{path}"), path]
            })
            .collect();
        let mut kept = kept;
        kept.sort();
        assert_eq!(reused, kept, "every other tile stays where the version before has it");
        let Target::Dir(dir) = &fixture.target else { unreachable!() };
        let path = format!("16/{}.tif", first.replace('-', "/"));
        assert!(dir.join("sources/terrain/2026-10-02").join(&path).is_file());
        assert!(!dir.join("sources/terrain/2026-10-02").join(&kept[0]).exists());
    }

    #[test]
    fn a_union_has_its_own_countries_else_those_of_its_parts_and_none_when_a_part_names_none() {
        let region = |text: &str| text.to_string();
        let definitions = |box_countries: &str| -> BTreeMap<String, String> {
            [
                ("both".to_string(), region("name = \"Both\"\nkind = \"union\"\nunion = [\"swiss\", \"east\"]\n")),
                ("swiss".to_string(), region("name = \"Swiss\"\nkind = \"geofabrik\"\nareas = [\"europe/switzerland\"]\ncountries = [\"CH\", \"LI\"]\n")),
                ("east".to_string(), region(&format!("name = \"East\"\nkind = \"box\"\nbox = [9.5, 46.8, 10.2, 47.6]\n{box_countries}"))),
            ]
            .into()
        };
        let scope = |box_countries: &str| Scope { region: "both".into(), definitions: definitions(box_countries) };
        let (_, countries) = area(Some(&scope("countries = [\"AT\", \"CH\"]\n"))).unwrap();
        assert_eq!(countries, ["AT", "CH", "LI"]);
        let (_, countries) = area(Some(&scope(""))).unwrap();
        assert!(countries.is_empty(), "a part without countries reads every country: {countries:?}");
        let mut named = scope("");
        named.definitions.insert(
            "both".into(),
            region("name = \"Both\"\nkind = \"union\"\nunion = [\"swiss\", \"east\"]\ncountries = [\"CH\", \"AT\"]\n"),
        );
        let (_, countries) = area(Some(&named)).unwrap();
        assert_eq!(countries, ["CH", "AT"], "the union's own list wins, as in the planner");
    }

    #[test]
    fn a_region_of_gb_reads_the_models_of_england_scotland_and_wales() {
        let gb = ["GB".to_string()];
        let read: Vec<&str> = models(obc_data::sources::all(), None, &gb).map(key).collect();
        assert_eq!(read, ["gb-eng", "gb-sct", "gb-wls"]);
    }
}
