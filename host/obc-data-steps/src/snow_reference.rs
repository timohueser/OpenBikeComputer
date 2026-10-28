//! The producer of `snow`: the tiles of zoom 6 to 12 of the snow archive of each zoom 6 block of its
//! region, `<season>/<x>/<y>.pmtiles` for each season of `snow.seasons` (`specs/obc-data.md`). A
//! version extends the version before: it makes only the files that the version before lacks, as
//! the data of a past season does not change. `tools/planner_snow.py prepare` writes the files of
//! one block, from the MODIS files of the archive dataset `modis-snow`, read in place.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::process::Command;

use obc_bake::coverage::Coverage;
use obc_data::fetch::archive::{self, Manifest};
use obc_data::prepared::{Job, Plan, Prepared, Scope};
use obc_data::product::Wanted;
use obc_data::regions::{Area, Bbox};
use obc_data::sources::Source;
use planner_router_build::grid::tile_bounds;

use crate::device::box_poly;
use crate::prepared_tiles::{self, outlines, poly, read_on_day, regions, reuse, Tile, BATCH, RUNNING};

/// The id of the prepared source.
pub const ID: &str = "snow";
pub const FORMAT: u32 = 1;
const BLOCK_ZOOM: u32 = 6;
const HR_WSI: &str = "hr-wsi";
/// The archive dataset and the source of the MODIS files.
const MODIS: &str = "modis-snow";
const CANOPY: &str = "hansen-gfc";
/// A file of a block-season, about the largest of the Alpine blocks of the DACH snow archive.
const FILE_BYTES: u64 = 160_000_000;
/// The MODIS sinusoidal grid: its sphere, its tiles of 2400 × 2400 pixels, and its north-west corner.
const MODIS_R: f64 = 6_371_007.181;
const MODIS_TILE_M: f64 = 1_111_950.519_766_523_3;
const MODIS_X0: f64 = -20_015_109.355_798;
const MODIS_Y0: f64 = 10_007_554.677_899;

pub struct SnowReference {
    /// `tools/planner_snow.py`, as a command in the repository root.
    pub tool: fn(&Path) -> Result<Command, String>,
    /// The archive roots, where the dataset `modis-snow` is.
    pub roots: fn() -> Vec<PathBuf>,
}

pub const SNOW_REFERENCE: SnowReference = SnowReference { tool, roots: archive::roots };

fn tool(root: &Path) -> Result<Command, String> {
    let mut command = Command::new("uv");
    command.args(["run", "--locked", "--offline", "--no-default-groups", "--no-python-downloads"]);
    command.args(["--group", "planner-snow", "python", "-m", "tools.planner_snow"]);
    command.env("UV_PYTHON", obc_data::engine::python_executable(root)?).env("UV_NO_SYNC", "0");
    Ok(command)
}

impl Prepared for SnowReference {
    fn name(&self) -> &'static str {
        ID
    }

    fn format(&self) -> u32 {
        FORMAT
    }

    fn revision(&self) -> u32 {
        1
    }

    /// `hr-wsi`, the `hansen-gfc` tiles of the blocks of a region of boxes, and the polygons of the
    /// Geofabrik leaves of the region. The files of the blocks of a region of boxes are the files
    /// at most.
    fn plan(
        &self,
        planning: &obc_data::prepared::Planning,
        _: &Source,
        scope: Option<&Scope>,
        _: &[String],
    ) -> Result<Plan, String> {
        modis_manifest(&(self.roots)())?;
        let mut inputs = vec![(Wanted { source: HR_WSI.into(), version: None, params: Vec::new() }, None).into()];
        let mut held = Some(BTreeSet::new());
        match scope {
            None => held = Some(every()),
            Some(scope) => {
                let (regions, id) = regions(scope)?;
                for leaf in regions.leaves(&id)? {
                    match &regions.get(leaf).expect("a leaf is a region").area {
                        Area::Box { bbox } => {
                            if let Some(held) = &mut held {
                                held.extend(blocks(&Coverage::parse_poly(&box_poly(bbox))?)?);
                            }
                        }
                        Area::Geofabrik { areas } => {
                            inputs.extend(areas.iter().map(|area| (poly(area, None), None).into()));
                            held = None;
                        }
                        Area::Union { .. } => {}
                    }
                }
            }
        }
        if let Some(blocks) = &held {
            let tiles: BTreeSet<String> = blocks.iter().flat_map(|&block| canopy_tiles(block_bounds(block))).collect();
            inputs.push((canopy(planning.sources, tiles)?, None).into());
        }
        let seasons = u64::try_from(seasons()?.count()).expect("few seasons");
        // The files of the programs that run at once, and a batch that waits for its move into the home.
        let running = held.as_ref().map_or(RUNNING, |blocks| blocks.len().min(RUNNING)) as u64;
        let work_bytes = (running * seasons + BATCH as u64) * FILE_BYTES;
        Ok(Plan { inputs, files: held.map(|blocks| blocks.len() as u64 * seasons), work_bytes })
    }

    /// The files of each block of the region that the version before lacks, made once and moved into
    /// the home with the batch of their block; every file of the version before stays.
    fn prepare(&self, job: &mut Job) -> Result<(), String> {
        let previous = job.extend(FORMAT)?;
        let held: BTreeSet<&str> = previous.iter().flat_map(|manifest| &manifest.files).map(|f| &f.path[..]).collect();
        let region = match outlines(job)? {
            Some(outlines) => {
                let mut region = BTreeSet::new();
                for outline in &outlines {
                    region.extend(blocks(outline)?);
                }
                region
            }
            None => every(),
        };
        let (root, manifest) = modis_manifest(&(self.roots)())?;
        let lists = job.state.join("modis");
        std::fs::create_dir_all(&lists).map_err(|e| format!("{}: {e}", lists.display()))?;
        let (mut tiles, mut args, mut read) = (Vec::new(), BTreeMap::new(), BTreeMap::new());
        for block in region {
            let name = format!("{}-{}", block.0, block.1);
            let lacking: Vec<u16> = seasons()?.filter(|&season| !held.contains(&path(season, block)[..])).collect();
            if lacking.is_empty() {
                continue;
            }
            let (_, canopy) =
                job.fetch(&canopy(job.sources(), canopy_tiles(block_bounds(block)).into_iter().collect())?)?;
            let modis: Vec<_> = modis_files(&manifest, block).collect();
            let list = lists.join(format!("{name}.txt"));
            let text: String = modis.iter().map(|file| format!("{}\n", root.join(&file.path).display())).collect();
            obc_data::store::write_atomic(&list, text.as_bytes())?;
            read.extend(modis.iter().map(|file| (file.path.clone(), file.sha256.clone())));
            let seasons = lacking.iter().map(u16::to_string).collect::<Vec<_>>().join(",");
            let canopy = canopy.values().map(|file| file.display().to_string()).collect::<Vec<_>>().join(",");
            let named =
                ["--block", &name, "--seasons", &seasons, "--modis", &list.display().to_string(), "--canopy", &canopy];
            args.insert(name.clone(), named.map(String::from));
            let files =
                lacking.iter().map(|&season| (path(season, block), format!("{name}.{season}.sha256"))).collect();
            tiles.push(Tile { name, files, marks: Vec::new() });
        }
        let digests = prepared_tiles::make(job, &tiles, |job, name| {
            let mut command = (self.tool)(job.root)?;
            command.arg("prepare").args(&args[name]).arg("--out").arg(&job.work);
            Ok(command)
        })?;
        reuse(job, previous.as_ref(), |path| !digests.contains_key(path))?;
        if !digests.is_empty() {
            read_on_day(job, HR_WSI, digests)?;
            let version = manifest.version.clone().or(manifest.got.clone()).unwrap_or_default();
            job.input(MODIS, &version, read)?;
        }
        Ok(())
    }
}

/// The root that holds the archive dataset `modis-snow`, and its manifest.
fn modis_manifest(roots: &[PathBuf]) -> Result<(PathBuf, Manifest), String> {
    let fix = format!("Fix: obc data archive get {MODIS}, and name its root in OBC_DATA_ARCHIVE");
    let root = obc_data::datasets::holder(roots, MODIS).ok_or_else(|| format!("no archive holds {MODIS}. {fix}"))?;
    let manifest = Manifest::read(&root.join("manifest").join(format!("{MODIS}.json")))?;
    Ok((root.clone(), manifest))
}

/// The files of `manifest` whose MODIS tile `block` meets with one MODIS pixel of margin; the
/// program takes each file whose tile meets its grid.
fn modis_files(manifest: &Manifest, block: (u32, u32)) -> impl Iterator<Item = &archive::Listed> {
    let (columns, rows) = modis_tiles(block);
    manifest.files.iter().filter(move |file| {
        // `MOD10A1.A2016245.h18v04.061.…`: the tile is the third part of the name.
        let name = file.path.rsplit('/').next().unwrap_or_default();
        let tile = name.split('.').nth(2).unwrap_or_default();
        let number = |range: std::ops::Range<usize>| tile.get(range).and_then(|text| text.parse::<u32>().ok());
        match (tile.starts_with('h'), number(1..3), number(4..6)) {
            (true, Some(h), Some(v)) => columns.contains(&h) && rows.contains(&v),
            _ => false,
        }
    })
}

/// The columns h and the rows v of the MODIS tiles that block `(x, y)` meets with one MODIS pixel of
/// margin. A sinusoidal x is widest at the latitude nearest the equator.
fn modis_tiles(block: (u32, u32)) -> (RangeInclusive<u32>, RangeInclusive<u32>) {
    let pixel = MODIS_TILE_M / 2400.0;
    let [west, south, east, north] = block_bounds(block);
    let equator = 0f64.clamp(south, north);
    let xs = [west, east].map(|lon| MODIS_R * lon.to_radians() * equator.to_radians().cos());
    let tile = |metres: f64, last: u32| ((metres / MODIS_TILE_M).floor().max(0.0) as u32).min(last);
    let columns = tile(xs[0] - pixel - MODIS_X0, 35)..=tile(xs[1] + pixel - MODIS_X0, 35);
    let rows = tile(MODIS_Y0 - MODIS_R * north.to_radians() - pixel, 17)
        ..=tile(MODIS_Y0 - MODIS_R * south.to_radians() + pixel, 17);
    (columns, rows)
}

/// The `hansen-gfc` fetch of `tiles`, at the `start` of its row in `sources`.
fn canopy(sources: &[Source], tiles: BTreeSet<String>) -> Result<Wanted, String> {
    let row = sources.iter().find(|source| source.id == CANOPY);
    let version = row.and_then(|row| row.start.clone()).ok_or_else(|| format!("`{CANOPY}` has no `start`"))?;
    let params = tiles.into_iter().map(|tile| ("tile".into(), tile)).collect();
    Ok(Wanted { source: CANOPY.into(), version: Some(version), params })
}

/// The 10° Hansen GFC tiles that `bounds` overlaps with one MODIS pixel, 1/240° of latitude, of
/// margin: the MODIS grid of a block reaches one pixel past it. Named by their north-west corner.
fn canopy_tiles([west, south, east, north]: [f64; 4]) -> Vec<String> {
    let pad = 1.0 / 240.0;
    let widen = pad / south.abs().max(north.abs()).to_radians().cos();
    let [west, south, east, north] = [west - widen, south - pad, east + widen, north + pad];
    let lats = (south / 10.0).floor() as i32 + 1..=(north / 10.0).ceil() as i32;
    let lons = (west / 10.0).floor() as i32..(east / 10.0).ceil() as i32;
    let name = |lat: i32, lon: i32| {
        let (ns, ew) = (if lat >= 0 { 'N' } else { 'S' }, if lon >= 0 { 'E' } else { 'W' });
        format!("{:02}{ns}_{:03}{ew}", (lat * 10).abs(), (lon * 10).abs())
    };
    lats.flat_map(|lat| lons.clone().map(move |lon| name(lat, lon))).collect()
}

/// The seasons of `snow.seasons` of `data/planner.toml`.
fn seasons() -> Result<RangeInclusive<u16>, String> {
    let [first, last] = crate::planner::snow_seasons()?;
    Ok(first..=last)
}

fn block_bounds((x, y): (u32, u32)) -> [f64; 4] {
    tile_bounds(BLOCK_ZOOM, x, y)
}

/// The blocks that `outline` meets, as `(x, y)`.
pub(crate) fn blocks(outline: &Coverage) -> Result<BTreeSet<(u32, u32)>, String> {
    let (west, south, east, north) = outline.bbox();
    let degrees = |udeg: i64| udeg as f64 / 1e6;
    let ((left, top), (right, bottom)) = (
        planner_router_build::grid::mercator(degrees(west), degrees(north), BLOCK_ZOOM),
        planner_router_build::grid::mercator(degrees(east), degrees(south), BLOCK_ZOOM),
    );
    let last = f64::from((1u32 << BLOCK_ZOOM) - 1);
    let index = |value: f64| value.floor().clamp(0.0, last) as u32;
    let mut found = BTreeSet::new();
    for x in index(left)..=index(right) {
        for y in index(top)..=index(bottom) {
            let [west, south, east, north] = block_bounds((x, y));
            let block = Coverage::parse_poly(&box_poly(&Bbox { west, south, east, north }))?;
            if outline.intersection(&block)?.is_some() {
                found.insert((x, y));
            }
        }
    }
    Ok(found)
}

/// The files of `blocks` for each season of `seasons`.
pub fn files(blocks: &BTreeSet<(u32, u32)>, seasons: RangeInclusive<u16>) -> BTreeSet<String> {
    blocks.iter().flat_map(|&block| seasons.clone().map(move |season| path(season, block))).collect()
}

/// The path of the file of `season` of block `(x, y)`.
fn path(season: u16, (x, y): (u32, u32)) -> String {
    format!("{season}/{x}/{y}.pmtiles")
}

/// Every block, for a version of everything.
fn every() -> BTreeSet<(u32, u32)> {
    let side = 1u32 << BLOCK_ZOOM;
    (0..side).flat_map(|x| (0..side).map(move |y| (x, y))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::tests::{fetched, temp};
    use obc_data::engine::runs::Run;
    use obc_data::fetch::http::Http;
    use obc_data::prepared::{Manifest, Opening};
    use obc_data::sources::Registry;
    use obc_data::store::Store;
    use std::sync::OnceLock;

    const SOURCES: &str = r#"
        [[source]]
        id = "snow"
        kind = "data"
        licence = "CC-BY-4.0"
        attribution = "Test"
        fetch = { kind = "prepared" }
        version = "date"
        keep = "prepared"
        redistribute = true

        [[source]]
        id = "hansen-gfc"
        kind = "data"
        licence = "CC-BY-4.0"
        attribution = "Test"
        fetch = { kind = "http", url = "https://example.org/{tile}.tif" }
        version = "release"
        start = "v1"
        keep = "archive"
        redistribute = true
    "#;

    /// The tool as a script: each file of a block holds its season and block, and its mark the
    /// same; `made` gets the block, its seasons and the MODIS files of its list.
    const TOOL: &str = r#"block=$3; shift 3
while [ $# -gt 0 ]; do case "$1" in --seasons) seasons=$2; shift 2;; --modis) modis=$2; shift 2;; --canopy) shift 2;; --out) out=$2; shift 2;; esac; done
x=${block%-*}; y=${block#*-}
for season in $(echo "$seasons" | tr , ' '); do mkdir -p "$out/$season/$x" && echo "$season $block" > "$out/$season/$x/$y.pmtiles" && echo "$season $block" > "$out/$block.$season.sha256"; done
echo "$block $seasons $(xargs -n1 basename < "$modis" | tr '\n' ' ')" >> made
"#;

    static ARCHIVE: OnceLock<PathBuf> = OnceLock::new();

    fn script(_: &Path) -> Result<Command, String> {
        let mut command = Command::new("sh");
        command.arg("tool.sh");
        Ok(command)
    }

    fn roots() -> Vec<PathBuf> {
        vec![ARCHIVE.get().expect("the test sets the archive").clone()]
    }

    fn scope(id: &str, west: f64, east: f64) -> Scope {
        let text = format!("name = \"{id}\"\nkind = \"box\"\nbox = [{west}, 47.0, {east}, 47.1]\n");
        Scope { region: id.into(), definitions: [(id.into(), text)].into() }
    }

    #[test]
    fn a_version_makes_the_seasons_of_its_blocks_that_the_version_before_lacks() {
        let temp = temp("snow");
        let root = temp.0.join("repository");
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/sources.toml"), SOURCES).unwrap();
        std::fs::write(root.join("tool.sh"), TOOL).unwrap();
        // The dataset `modis-snow`: a file of the MODIS tile of the Alps, h18v04, and one east of it.
        let archive = temp.0.join("archive");
        let alps = "MOD10A1.A2016245.h18v04.061.2021338033043_NDSI_Snow_Cover.tif";
        let listed = [alps, "MOD10A1.A2016245.h19v04.061.1.hdf"]
            .map(|name| serde_json::json!({"path": format!("mirror/example.org/{name}"), "bytes": 1, "sha256": name}));
        std::fs::create_dir_all(archive.join("manifest")).unwrap();
        let manifest = serde_json::json!({"version": "2026-10-20", "files": listed});
        std::fs::write(archive.join("manifest/modis-snow.json"), manifest.to_string()).unwrap();
        ARCHIVE.set(archive.clone()).unwrap();
        let store = Store::at(temp.0.join("store"));
        for tiles in [&["50N_000E", "50N_010E"][..], &["50N_010E"]] {
            let params: Vec<_> = tiles.iter().map(|tile| ("tile".to_string(), tile.to_string())).collect();
            let files: Vec<_> = tiles.iter().map(|tile| (format!("{tile}.tif"), "cover".to_string())).collect();
            fetched(&store, CANOPY, "v1", &params, &files);
        }
        let sources = Registry::load(&root).unwrap().sources;
        let (producer, http) = (SnowReference { tool: script, roots }, Http::new());
        let version = |version: &str, scope: Scope| {
            let mut run = Run::create(&store, ["source", "prepare", ID]).unwrap();
            let opening = Opening { root: &root, store: &store, http: &http, sources: &sources, home: &archive };
            let mut job = Job::open(opening, (ID, version), Some(scope), &[], &mut run).unwrap();
            producer.prepare(&mut job).unwrap();
            let manifest = Manifest {
                source: ID.into(),
                version: version.into(),
                format: FORMAT,
                revision: 1,
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
            manifest
        };
        let of = |blocks: &[(u32, u32)]| files(&blocks.iter().copied().collect(), seasons().unwrap());
        let paths = |manifest: &Manifest| manifest.files.iter().map(|file| file.path.clone()).collect::<BTreeSet<_>>();

        // 8 °E, 47 °N is in block 33, 22, and 12 °E in block 34, 22.
        let first = version("2026-10-01", scope("west", 8.0, 8.1));
        assert_eq!(paths(&first), of(&[(33, 22)]));
        let second = version("2026-10-02", scope("east", 12.0, 12.1));
        assert_eq!(paths(&second), of(&[(33, 22), (34, 22)]));
        assert_eq!(second.reused.keys().cloned().collect::<BTreeSet<_>>(), of(&[(33, 22)]));
        assert_eq!(second.extends, [first.scope]);
        // Block 34, 22 also meets the MODIS tile h19v04.
        let all = seasons().unwrap().map(|season| season.to_string()).collect::<Vec<_>>().join(",");
        let made = std::fs::read_to_string(root.join("made")).unwrap();
        let east = format!("{alps} MOD10A1.A2016245.h19v04.061.1.hdf");
        assert_eq!(made, format!("33-22 {all} {alps} \n34-22 {all} {east} \n"), "a block is made once");

        // A version of everything has every block.
        let planning = obc_data::prepared::Planning { store: &store, http: &http, sources: &sources };
        let everything = producer.plan(&planning, &sources[0], None, &[]).unwrap();
        assert_eq!(everything.files, Some(64 * 64 * seasons().unwrap().count() as u64));
    }
}
