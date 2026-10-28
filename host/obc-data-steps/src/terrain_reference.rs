//! The producer of `terrain`: the national terrain models `dtm-<key>` as blocks of 4 × 4 tiles of
//! the reference archive (`host/obc-dem/reference/README.md`), `18/<bi>/<bj>.tif` of the maximum
//! and `mean/18/<bi>/<bj>.tif` of the mean, and one `index.json`. A version makes each block of its
//! region from every model with a tile in it, so the pixels of a block do not depend on the region,
//! and keeps every other block of the version before without a copy. The index states the GLO-30
//! bias of each model (`crate::terrain_bias`).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;

use obc_bake::coverage::Coverage;
use obc_data::prepared::{day, Job, Manifest, Plan, Prepared, Scope};
use obc_data::product::Wanted;
use obc_data::regions::Bbox;
use obc_data::sources::Source;
use obc_dem::reference::TILE_LOG2;
use obc_map_core::grid::CellId;

use crate::device::box_poly;
use crate::prepared_tiles::{self, outline, outlines, poly, read_on_day, regions, reuse, Tile};
use crate::terrain_bias::Made;

/// The id of the prepared source.
pub const ID: &str = "terrain";
pub(crate) const FORMAT: u32 = 4;
/// The side of a block as `log2(µdeg)`: 4 × 4 archive tiles.
const BLOCK_LOG2: u32 = TILE_LOG2 as u32 + 2;

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
        FORMAT
    }

    fn revision(&self) -> u32 {
        1
    }

    /// The models with a tile in a block that the boxes of the region meet, else those of its
    /// countries and each model whose `extent` meets one of theirs, and the polygons of their areas.
    /// The two pools of the blocks that the boxes meet, and `index.json`, are the files at most.
    fn plan(
        &self,
        planning: &obc_data::prepared::Planning,
        _: &Source,
        scope: Option<&Scope>,
        _: &[String],
    ) -> Result<Plan, String> {
        let (boxes, countries) = area(scope)?;
        let met = match &boxes {
            Some(boxes) => {
                let mut met = BTreeSet::new();
                for bbox in boxes {
                    met.extend(Coverage::parse_poly(&box_poly(bbox))?.cells(BLOCK_LOG2));
                }
                Some(met)
            }
            None => None,
        };
        let extents: Vec<[f64; 4]> = models(planning.sources)
            .filter(|model| countries.contains(&country(model)))
            .filter_map(|model| model.extent)
            .collect();
        let (mut inputs, mut blocks) = (Vec::new(), BTreeSet::new());
        for model in models(planning.sources) {
            if met.is_some() {
                let tiles = model_tiles(model, met.as_ref())?;
                if tiles.is_empty() {
                    continue;
                }
                blocks.extend(tiles.iter().map(block));
            } else if !countries.is_empty() && !extents.iter().any(|extent| model.meets(*extent)) {
                continue;
            }
            inputs.push(Wanted { source: model.id.clone(), version: None, params: Vec::new() });
            inputs.extend(model.areas.iter().map(|area| poly(area, None)));
        }
        let inputs = inputs.into_iter().map(|wanted| (wanted, None).into()).collect();
        Ok(Plan { inputs, files: met.map(|_| 2 * blocks.len() as u64 + 1), work_bytes: 0 })
    }

    /// Each block of the region: the models of each of its tiles fetched once and folded, then its
    /// two pools moved into the home with the batch of the block. `state/tiles/` keeps the pixel
    /// digests and the tiles of a made block, or `.none` for a block without a height, so a resume
    /// fetches neither again.
    fn prepare(&self, job: &mut Job) -> Result<(), String> {
        let previous = job.extend(FORMAT)?;
        // The blocks that the outlines of the region meet; every block for a version of everything.
        let met: Option<BTreeSet<CellId>> =
            outlines(job)?.map(|outlines| outlines.iter().flat_map(|outline| outline.cells(BLOCK_LOG2)).collect());
        // The models of each tile of those blocks, in the order of `data/sources.toml`.
        let mut keys: BTreeMap<CellId, Vec<String>> = BTreeMap::new();
        for model in models(job.sources()) {
            let mut tiles = model_tiles(model, met.as_ref())?;
            if tiles.is_empty() {
                continue;
            }
            if !model.areas.is_empty() {
                let mut inside = BTreeSet::new();
                for area in &model.areas {
                    inside.extend(outline(job, area)?.cells(TILE_LOG2.into()));
                }
                tiles.retain(|tile| inside.contains(tile));
            }
            for tile in tiles {
                keys.entry(tile).or_default().push(key(model).into());
            }
        }
        let region: BTreeSet<CellId> = keys.keys().map(block).collect();
        let name = |block: &CellId| format!("{:04}-{:04}", block.i, block.j);
        let tiles: Vec<Tile> = region
            .iter()
            .map(|at| {
                let (name, (ti, tj)) = (name(at), (4 * at.i, 4 * at.j));
                let files = vec![
                    (block_file("mean/", ti, tj), format!("{name}.mean.sha256")),
                    (block_file("", ti, tj), format!("{name}.sha256")),
                ];
                Tile { name: name.clone(), files, marks: vec![format!("{name}.tiles")] }
            })
            .collect();
        let mut of: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (tile, models) in &keys {
            of.entry(name(&block(tile))).or_default().push(format!("{:04}-{:04}={}", tile.i, tile.j, models.join(",")));
        }
        let digests = prepared_tiles::make(job, &tiles, |job, name| {
            let mut command = (self.ingest)(job.root)?;
            command.arg("fetch").args(&of[name]).args(["--block", name, "--work"]);
            // A folder per block: `fetch` removes its `--work` at the end, and blocks run at once.
            command.arg(job.state.join("raw").join(name)).arg("--out").arg(&job.work);
            // The summit check of each model, once per prepare, and a file a whole region is in.
            command.arg("--shared").arg(job.state.join("shared"));
            Ok(command)
        })?;
        // A block outside the region is the block of the version before.
        let remade: BTreeSet<&str> = tiles.iter().flat_map(|tile| &tile.files).map(|(path, _)| path.as_str()).collect();
        reuse(job, previous.as_ref(), |path| path.ends_with(".tif") && !remade.contains(path))?;
        let marks = job.state.join("tiles");
        let mut made: BTreeMap<String, Made> = BTreeMap::new();
        for tile in tiles.iter().filter(|tile| digests.contains_key(&tile.files[1].0)) {
            let path = marks.join(&tile.marks[0]);
            let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let tiles: BTreeMap<String, Made> =
                serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            made.extend(tiles);
        }
        let before = previous.as_ref().map(|manifest| index(job, manifest)).transpose()?;
        let biases = crate::terrain_bias::measure(job, (previous.as_ref(), before.as_ref()), &made, self.glo30)?;
        let fetched: BTreeSet<&String> = keys.values().flatten().collect();
        let fetched: Vec<&str> = fetched.into_iter().map(String::as_str).collect();
        // A model has no versions of its own: the day of the prepare names what it read.
        let version = day(&job.version).to_string();
        let sources: BTreeMap<&String, &Vec<String>> = made.iter().map(|(tile, made)| (tile, &made.sources)).collect();
        let blocks: Vec<String> = region.iter().map(|block| format!("{:04}/{:04}", block.i, block.j)).collect();
        let mut command = (self.ingest)(job.root)?;
        command.args(["index-terrain", "--fetched", &version, "--models", &fetched.join(",")]);
        for (flag, file, json) in [
            ("--keys", "keys.json", serde_json::to_vec(&sources)),
            ("--blocks", "blocks.json", serde_json::to_vec(&blocks)),
            ("--biases", "biases.json", serde_json::to_vec(&biases)),
        ] {
            let path = job.state.join(file);
            obc_data::store::write_atomic(&path, &json.expect("the index inputs serialize"))?;
            command.arg(flag).arg(path);
        }
        if let Some(previous) = &previous {
            command.arg("--base").arg(job.earlier_file(previous, "index.json")?);
        }
        job.program(command.arg("--out").arg(job.work.join("index.json")))?;
        // The sources of the tiles of each block that the run made, by the path of its maximum.
        let mut held: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
        for (tile, made) in &made {
            let (ti, tj) = tile
                .split_once('/')
                .and_then(|(i, j)| Some((i.parse::<i64>().ok()?, j.parse::<i64>().ok()?)))
                .ok_or_else(|| format!("a mark names the tile `{tile}`"))?;
            held.entry(block_file("", ti, tj)).or_default().extend(made.sources.iter().map(String::as_str));
        }
        for model in &fetched {
            let read: BTreeMap<String, String> = tiles
                .iter()
                .filter(|tile| held.get(&tile.files[1].0).is_some_and(|sources| sources.contains(model)))
                .flat_map(|tile| &tile.files)
                .filter_map(|(path, _)| Some((path.clone(), digests.get(path)?.clone())))
                .collect();
            read_on_day(job, &format!("dtm-{model}"), read)?;
        }
        job.finish(&["index.json".into()])
    }
}

/// The file of the pool `prefix` (`""` or `"mean/"`) of the block that holds tile `(ti, tj)`.
pub(crate) fn block_file(prefix: &str, ti: impl Into<i64>, tj: impl Into<i64>) -> String {
    format!("{prefix}18/{:04}/{:04}.tif", ti.into() >> 2, tj.into() >> 2)
}

/// The block that holds `tile`.
fn block(tile: &CellId) -> CellId {
    CellId { log2: BLOCK_LOG2, i: tile.i >> 2, j: tile.j >> 2 }
}

/// The box of each leaf of the region of `scope` and its countries; no boxes for a version of
/// everything or a region with a Geofabrik leaf. A region that names no countries has those of its
/// leaves, and none when a leaf names none: that leaf reads every country.
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

/// The national models of `sources` that may be given to others.
fn models(sources: &[Source]) -> impl Iterator<Item = &Source> {
    sources.iter().filter(|source| source.id.starts_with("dtm-") && source.redistribute)
}

/// The archive tiles that the `extent` of `model` meets in the blocks `met`; all of them without
/// `met`.
fn model_tiles(model: &Source, met: Option<&BTreeSet<CellId>>) -> Result<BTreeSet<CellId>, String> {
    let [west, south, east, north] = model.extent.ok_or_else(|| format!("`{}` has no `extent`", model.id))?;
    let extent = Coverage::parse_poly(&box_poly(&Bbox { west, south, east, north }))?;
    let Some(met) = met else { return Ok(extent.cells(TILE_LOG2.into())) };
    // A model far from the region costs one pass over its blocks, not over its tiles.
    if extent.cells(BLOCK_LOG2).is_disjoint(met) {
        return Ok(BTreeSet::new());
    }
    let mut tiles = extent.cells(TILE_LOG2.into());
    tiles.retain(|tile| met.contains(&block(tile)));
    Ok(tiles)
}

/// The country of a national model: the first part of its key is the ISO 3166-1 code, so
/// `dtm-de-by` is of `DE`.
fn country(model: &Source) -> String {
    key(model).split('-').next().unwrap_or_default().to_uppercase()
}

/// The key of a national model: `dtm-ch` is `ch`.
fn key(model: &Source) -> &str {
    &model.id["dtm-".len()..]
}

/// The sources of each tile of the index of the version before, and the facts of each source.
#[derive(serde::Deserialize)]
pub(crate) struct Index {
    pub contributors: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub sources: BTreeMap<String, serde_json::Value>,
}

fn index(job: &Job, manifest: &Manifest) -> Result<Index, String> {
    let path = job.earlier_file(manifest, "index.json")?;
    serde_json::from_slice(&fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| format!("{}@{} index.json: {e}", job.id, manifest.version))
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
    use obc_dem::fetch::TileId;
    use std::path::PathBuf;

    const SOURCES: &str = r#"
        [[source]]
        id = "dtm-xx"
        kind = "data"
        licence = "CC0-1.0"
        attribution = "Test"
        fetch = { kind = "dtm", url = "https://example.org/xx" }
        extent = [8.0, 46.0, 9.1, 46.9]
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
    /// `works` once it holds a raw file there. A block with a file `none-<block>` has no height, and
    /// one with `fail-<block>` fails. One with `wait-<block>` waits until `works` has as many lines
    /// as that file says, and one with `hold-<block>` runs until a signal stops it. Each tile of a
    /// block gets its models as sources. `index-terrain` writes the sources that it gets and a bias of
    /// `xx`, and logs `over` with a `--base`, unless `fail-index` exists.
    const INGEST: &str = r#"case "$1" in
fetch)
    shift; tiles=""; while [ "$1" != --block ]; do tiles="$tiles $1"; shift; done
    b="$2"; work="$4"; out="$6"
    echo "fetch $b$tiles" >> calls
    # As `ingest.py fetch` does, it removes its `--work` when it ends, also on SIGTERM.
    trap 'rm -rf "$work"' EXIT && trap 'exit 143' TERM
    mkdir -p "$work" && touch "$work/raw.tif" && echo "$work" >> works
    if [ -e "wait-$b" ]; then
        n=0; until [ "$(wc -l < works)" -ge "$(cat "wait-$b")" ]; do n=$((n + 1)); [ $n -gt 1000 ] && exit 2; sleep 0.01; done
    fi
    if [ -e "hold-$b" ]; then while :; do sleep 0.01; done; fi
    [ -e "fail-$b" ] && exit 1
    if [ -e "none-$b" ]; then touch "$out/$b.none"; exit 0; fi
    for pool in "" mean/; do mkdir -p "$out/${pool}18/${b%-*}" && echo "$pool$b$tiles" > "$out/${pool}18/${b%-*}/${b#*-}.tif"; done
    samples="$(yes 15000 | head -n 256 | tr '\n' ' ')"; sep=""; printf '{' > "$out/$b.tiles"
    for t in $tiles; do
        id="$(echo "${t%%=*}" | tr - /)"; keys="$(echo "${t#*=}" | sed 's/,/","/g')"
        printf '%s"%s": {"sources": ["%s"], "samples": "%s"}' "$sep" "$id" "$keys" "$samples" >> "$out/$b.tiles"; sep=","
    done
    echo '}' >> "$out/$b.tiles"
    echo "mean of $b" > "$out/$b.mean.sha256" && echo "pixels of $b" > "$out/$b.sha256" ;;
index-terrain)
    over=""; case " $* " in *" --base "*) over=" over" ;; esac
    echo "$1$over" >> calls
    [ -e fail-index ] && exit 1
    while [ $# -gt 0 ]; do case "$1" in --keys) keys="$2" ;; --out) out="$2" ;; esac; shift; done
    printf '{"contributors": %s, "sources": {"xx": {"glo30_bias_m": 0.5}}}' "$(cat "$keys")" > "$out" ;;
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

    /// A repository with [`INGEST`], a store and an archive root for `terrain`.
    struct Fixture {
        temp: Temp,
        sources: Vec<obc_data::sources::Source>,
        store: Store,
        archive: PathBuf,
        /// The blocks of the extent, in the order of the prepare.
        blocks: Vec<String>,
        cells: Vec<CellId>,
        /// The tiles of the extent in each block.
        tiles: BTreeMap<String, Vec<String>>,
    }

    impl Fixture {
        fn new(name: &str) -> Fixture {
            Fixture::with(name, "")
        }

        /// A fixture whose sources have the rows `more` too.
        fn with(name: &str, more: &str) -> Fixture {
            let temp = temp(name);
            let root = temp.0.join("repository");
            fs::create_dir_all(root.join("data")).unwrap();
            fs::write(root.join("data/sources.toml"), format!("{SOURCES}\n{more}")).unwrap();
            fs::write(root.join("ingest.sh"), INGEST).unwrap();
            let sources = Registry::load(&root).unwrap().sources;
            let extent = model_tiles(sources.iter().find(|source| source.id == "dtm-xx").unwrap(), None).unwrap();
            let cells: Vec<CellId> = extent.iter().map(block).collect::<BTreeSet<_>>().into_iter().collect();
            let blocks: Vec<String> = cells.iter().map(|block| format!("{:04}-{:04}", block.i, block.j)).collect();
            assert!(blocks.len() > RUNNING + 2, "{blocks:?}");
            let mut tiles: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for tile in &extent {
                let at = block(tile);
                tiles
                    .entry(format!("{:04}-{:04}", at.i, at.j))
                    .or_default()
                    .push(format!("{:04}-{:04}", tile.i, tile.j));
            }
            let (store, archive) = (Store::at(temp.0.join("store")), temp.0.join("archive"));
            Fixture { temp, sources, store, archive, blocks, cells, tiles }
        }

        fn root(&self) -> PathBuf {
            self.temp.0.join("repository")
        }

        fn state(&self) -> PathBuf {
            self.store.root().join("prepared/terrain/2026-10-01/state")
        }

        /// Write the file `<what>-<block k>` that changes what [`INGEST`] does.
        fn mark(&self, what: &str, k: usize, text: &str) {
            fs::write(self.root().join(format!("{what}-{}", self.blocks[k])), text).unwrap();
        }

        /// A run of the prepare, and the calls of the script, sorted: fetches run at once.
        fn prepare(&self) -> (Result<(), String>, Vec<String>) {
            let (done, calls, _, _) = self.version("2026-10-01", None);
            (done, calls)
        }

        /// A run of the prepare of `version` for `scope`: its end, the calls of the script, sorted,
        /// the reused files, complete in the archive when the run ended well, and the status of the run.
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
            let opening =
                Opening { root: &root, store: &self.store, http: &http, sources: &self.sources, home: &self.archive };
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
                obc_data::prepared::complete(&obc_data::prepared::folder(&self.archive, ID), &manifest).map(|_| ())
            });
            let mut calls: Vec<String> =
                fs::read_to_string(root.join("calls")).unwrap_or_default().lines().map(String::from).collect();
            calls.sort();
            run.flush().unwrap();
            let status = serde_json::from_slice(&fs::read(run.status_file()).unwrap()).unwrap();
            (done, calls, reused, status)
        }

        /// The call of the fetch of block `k`, with each of its tiles.
        fn fetch(&self, k: usize) -> String {
            let tiles: Vec<String> = self.tiles[&self.blocks[k]].iter().map(|tile| format!("{tile}=xx")).collect();
            format!("fetch {} {}", self.blocks[k], tiles.join(" "))
        }

        /// The calls of the fetches of blocks `k`, sorted.
        fn fetches(&self, k: impl Iterator<Item = usize>) -> Vec<String> {
            let mut calls: Vec<String> = k.map(|k| self.fetch(k)).collect();
            calls.sort();
            calls
        }

        /// Whether `done` failed in the program of block `k`, named with its progress.
        fn failed_in(&self, done: Result<(), String>, k: usize) -> bool {
            let label = format!("tile {}/{} {}: ", k + 1, self.blocks.len(), self.blocks[k]);
            done.is_err_and(|e| e.starts_with(&label))
        }
    }

    #[test]
    fn a_resume_fetches_no_block_that_it_has_and_the_index_holds_the_sources_of_each_tile() {
        let fixture = Fixture::new("terrain-reference");
        let blocks = &fixture.blocks;
        let last = blocks.len() - 1;
        // Blocks 0 and 1 end at once and free their slots for two more blocks. Block 2 fails once
        // those have started, and the failure stops the blocks that run until a signal stops them.
        let started = RUNNING + 2;
        fixture.mark("none", 0, "");
        fixture.mark("fail", 2, "");
        fixture.mark("wait", 2, &started.to_string());
        for k in 3..started {
            fixture.mark("hold", k, "");
        }
        let (done, calls) = fixture.prepare();
        assert!(fixture.failed_in(done, 2));
        assert_eq!(calls, fixture.fetches(0..started));

        // No block without a height is fetched, and the progress counts each block of the version.
        for what in ["fail", "wait"] {
            fs::remove_file(fixture.root().join(format!("{what}-{}", blocks[2]))).unwrap();
        }
        for block in &blocks[3..started] {
            fs::remove_file(fixture.root().join(format!("hold-{block}"))).unwrap();
        }
        fixture.mark("fail", last, "");
        let (done, calls) = fixture.prepare();
        assert!(fixture.failed_in(done, last));
        assert_eq!(calls, fixture.fetches(2..blocks.len()));

        // The blocks in `work/` are moved into the home with their digests.
        fs::remove_file(fixture.root().join(format!("fail-{}", blocks[last]))).unwrap();
        fs::write(fixture.root().join("fail-index"), "").unwrap();
        let (done, calls) = fixture.prepare();
        assert!(done.is_err());
        assert_eq!(calls, [fixture.fetches(last..blocks.len()), vec!["index-terrain".into()]].concat());

        fs::remove_file(fixture.root().join("fail-index")).unwrap();
        // A mark of a block that is not in the version.
        fs::write(fixture.state().join("tiles/9999-9999.sha256"), "pixels of 9999-9999\n").unwrap();
        let (done, calls) = fixture.prepare();
        done.unwrap();
        assert_eq!(calls, ["index-terrain"], "a resume fetches no block that the archive has");
        let version = fixture.archive.join("prepared/terrain/2026-10-01");
        let index: serde_json::Value = serde_json::from_slice(&fs::read(version.join("index.json")).unwrap()).unwrap();
        let index: BTreeMap<String, Vec<String>> = serde_json::from_value(index["contributors"].clone()).unwrap();
        let tiles: Vec<String> =
            blocks[1..].iter().flat_map(|block| &fixture.tiles[block]).map(|t| t.replace('-', "/")).collect();
        assert_eq!(index.keys().cloned().collect::<Vec<_>>(), {
            let mut tiles = tiles;
            tiles.sort();
            tiles
        });
        assert!(index.values().all(|sources| sources == &["xx"]), "the sources of each tile of the made blocks");
        for (k, block) in fixture.cells.iter().enumerate() {
            for prefix in ["", "mean/"] {
                let file = version.join(block_file(prefix, 4 * block.i, 4 * block.j));
                assert_eq!(file.exists(), k > 0, "{}", file.display());
            }
        }
        let file = version.join(block_file("mean/", 4 * fixture.cells[1].i, 4 * fixture.cells[1].j));
        assert!(fs::read_to_string(file)
            .unwrap()
            .starts_with(&format!("mean/{}", &fixture.fetch(1)["fetch ".len()..])));
    }

    #[test]
    fn fetches_at_once_have_a_raw_folder_each_and_a_failure_stops_the_others_which_remove_their_folders() {
        let fixture = Fixture::new("terrain-reference-raw");
        // Block 0 ends only once block 1 runs beside it, and the two free their slots for two more
        // blocks. Block 2 fails once those have started, and the blocks after it run until the
        // failure stops them.
        let started = RUNNING + 2;
        fixture.mark("wait", 0, "2");
        fixture.mark("wait", 2, &started.to_string());
        fixture.mark("fail", 2, "");
        for k in 3..started {
            fixture.mark("hold", k, "");
        }
        let (done, calls) = fixture.prepare();
        assert!(fixture.failed_in(done, 2));
        assert_eq!(calls, fixture.fetches(0..started));
        let mut works: Vec<String> =
            fs::read_to_string(fixture.root().join("works")).unwrap().lines().map(String::from).collect();
        works.sort();
        let raw = fixture.state().join("raw");
        let mut folders: Vec<String> =
            fixture.blocks[..started].iter().map(|block| raw.join(block).display().to_string()).collect();
        folders.sort();
        assert_eq!(works, folders);
        assert_eq!(fs::read_dir(&raw).unwrap().count(), 0, "each fetch removed its folder before the end");
    }

    #[test]
    fn a_block_that_ends_frees_its_slot_while_an_older_block_runs() {
        let fixture = Fixture::new("terrain-reference-slots");
        // Block 0 ends only once one block more than there are slots has started, so the younger
        // blocks that end must free their slots while it runs.
        fixture.mark("wait", 0, &(RUNNING + 1).to_string());
        let (done, calls) = fixture.prepare();
        done.unwrap();
        assert!(calls.contains(&fixture.fetch(RUNNING)), "{calls:?}");
    }

    #[test]
    fn a_version_makes_the_whole_blocks_of_its_region_again_and_keeps_the_others_of_the_version_before() {
        let fixture = Fixture::new("terrain-extend");
        let (done, _, _, _) = fixture.version("2026-10-01", None);
        done.unwrap();
        // The model has no bias yet, so its samples ask GLO-30 for the squares under them.
        let squares = fs::read_to_string(fixture.root().join("squares")).unwrap();
        assert!(squares.lines().any(|square| square == "Copernicus_DSM_COG_10_N46_00_E008_00_DEM"), "{squares}");
        // A box inside one tile of a full block, in a region of another country.
        let k = fixture.blocks.iter().position(|block| fixture.tiles[block].len() == 16).expect("a full block");
        let first = fixture.cells[k];
        let (west, south, east, north) =
            CellId::new(TILE_LOG2.into(), 4 * first.i + 1, 4 * first.j + 1).unwrap().square();
        let inner = |a: i64, b: i64| (a + (b - a) / 4) as f64 / 1e6;
        let definition = format!(
            "name = \"Box\"\nkind = \"box\"\nbox = [{}, {}, {}, {}]\ncountries = [\"DE\"]\n",
            inner(west, east),
            inner(south, north),
            inner(east, west),
            inner(north, south)
        );
        let scope = Scope { region: "box".into(), definitions: [("box".to_string(), definition)].into() };
        let (done, calls, reused, status) = fixture.version("2026-10-02", Some(scope));
        done.unwrap();
        // The run shows the block that it made as a step of `terrain`, with its time.
        assert_eq!(
            (&status["layers"][0]["layer"], &status["layers"][0]["done"], &status["layers"][0]["total"]),
            (&"terrain".into(), &1.into(), &1.into())
        );
        let step = &status["steps"][0];
        assert_eq!(step["step"], format!("terrain/{}", fixture.blocks[k]));
        assert!(step["wall_ms"].is_u64() && step["bytes_out"].as_u64() > Some(0), "{step}");
        assert_eq!(calls, [fixture.fetch(k), "index-terrain over".into()], "the whole block, from every model");
        let mut kept: Vec<String> = (0..fixture.blocks.len())
            .filter(|other| *other != k)
            .flat_map(|other| {
                let block = fixture.cells[other];
                ["", "mean/"].map(|prefix| block_file(prefix, 4 * block.i, 4 * block.j))
            })
            .collect();
        kept.sort();
        assert_eq!(reused, kept, "every other block stays where the version before has it");
        let version = fixture.archive.join("prepared/terrain/2026-10-02");
        assert!(version.join(block_file("", 4 * first.i, 4 * first.j)).is_file());
        assert!(!version.join(&kept[0]).exists());
    }

    /// A box region in the extent of `dtm-xx`, of Germany.
    fn boxed(west: f64, south: f64, east: f64, north: f64) -> Option<Scope> {
        let text =
            format!("name = \"Box\"\nkind = \"box\"\nbox = [{west}, {south}, {east}, {north}]\ncountries = [\"DE\"]\n");
        Some(Scope { region: "box".into(), definitions: [("box".to_string(), text)].into() })
    }

    /// `dtm-yy` lies east of `dtm-xx`, from 9.1° E. Block 1200/1058, 8.913° to 9.175° E and 46.147°
    /// to 46.410° N, holds tiles of both.
    const EAST: &str = r#"
        [[source]]
        id = "dtm-yy"
        kind = "data"
        licence = "CC0-1.0"
        attribution = "Test"
        fetch = { kind = "dtm", url = "https://example.org/yy" }
        extent = [9.1, 46.0, 9.5, 46.9]
        version = "date"
        keep = "prepared"
        redistribute = true
    "#;

    #[test]
    fn a_block_reads_each_model_with_a_tile_in_it_also_one_that_misses_the_region() {
        let fixture = Fixture::with("terrain-neighbour", EAST);
        // The box meets only block 1200/1058 and does not reach the extent of `dtm-yy`.
        let (done, calls, _, _) = fixture.version("2026-10-01", boxed(8.95, 46.30, 9.00, 46.35));
        done.unwrap();
        let [fetch, index] = &calls[..] else { panic!("{calls:?}") };
        assert_eq!(index, "index-terrain");
        assert!(fetch.starts_with("fetch 1200-1058 "), "{fetch}");
        let tiles: Vec<&str> = fetch.split(' ').skip(2).collect();
        assert!(tiles.iter().any(|tile| tile.ends_with("=xx,yy")), "a tile of both models: {fetch}");
        assert!(tiles.iter().any(|tile| tile.ends_with("=yy")), "a tile of `dtm-yy` alone: {fetch}");

        // Another region in the same block makes it again from the same models.
        let (done, again, _, _) = fixture.version("2026-10-02", boxed(8.92, 46.20, 8.94, 46.25));
        done.unwrap();
        assert_eq!(again, [fetch.clone(), "index-terrain over".into()]);
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
    fn the_models_of_gb_are_those_of_england_scotland_and_wales() {
        let read: Vec<&str> =
            models(obc_data::sources::all()).filter(|model| country(model) == "GB").map(key).collect();
        assert_eq!(read, ["gb-eng", "gb-sct", "gb-wls"]);
    }
}
