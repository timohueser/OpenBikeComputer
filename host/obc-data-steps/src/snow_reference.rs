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
/// The most days in a row that a MODIS tile of a block may lack a file in a season. Chosen: the
/// nearest clear day fills such a run as it fills a cloudy week; a longer run is a gap of the
/// source, such as the 67 days of 2025 that the Planetary Computer lacks.
const MAX_GAP_DAYS: i64 = 7;

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
        let defined = defined_tiles(job.root)?;
        let named: Vec<_> = manifest.files.iter().filter_map(|file| Some((file, modis_file(&file.path)?))).collect();
        let lists = job.state.join("modis");
        // The programs share the connections that CDSE allows a user: `CDSE_CONNECTIONS` of the tool.
        let slots = job.state.join("cdse");
        std::fs::create_dir_all(&lists).map_err(|e| format!("{}: {e}", lists.display()))?;
        let (mut tiles, mut args, mut read) = (Vec::new(), BTreeMap::new(), BTreeMap::new());
        for block in region {
            let name = format!("{}-{}", block.0, block.1);
            let lacking: Vec<u16> = seasons()?.filter(|&season| !held.contains(&path(season, block)[..])).collect();
            if lacking.is_empty() {
                continue;
            }
            let modis = modis_files(&named, block);
            modis_days(block, &modis, &defined, &lacking)?;
            let list = lists.join(format!("{name}.txt"));
            let text: String = modis.iter().map(|(file, _)| format!("{}\n", root.join(&file.path).display())).collect();
            obc_data::store::write_atomic(&list, text.as_bytes())?;
            read.extend(modis.iter().map(|(file, _)| (file.path.clone(), file.sha256.clone())));
            let (_, canopy) =
                job.fetch(&canopy(job.sources(), canopy_tiles(block_bounds(block)).into_iter().collect())?)?;
            let seasons = lacking.iter().map(u16::to_string).collect::<Vec<_>>().join(",");
            let canopy = canopy.values().map(|file| file.display().to_string()).collect::<Vec<_>>().join(",");
            let (list, slots) = (list.display().to_string(), slots.display().to_string());
            let named =
                ["--block", &name, "--seasons", &seasons, "--modis", &list, "--canopy", &canopy, "--slots", &slots];
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

/// A MODIS file of the dataset, with its tile `(h, v)` and its day from 1970-01-01.
type Named<'a> = (&'a archive::Listed, ((u32, u32), i64));

/// The files of `named`, each with its tile and day, whose tile `block` meets, as [`modis_tiles`]
/// says.
fn modis_files<'a>(named: &[Named<'a>], block: (u32, u32)) -> Vec<Named<'a>> {
    let (columns, rows) = modis_tiles(block);
    named.iter().filter(|(_, ((h, v), _))| columns.contains(h) && rows.contains(v)).copied().collect()
}

/// The MODIS tiles that `data/archive/modis-snow.toml` names, such as `.h18v04.`: the tiles of land
/// of the dataset. A tile that it does not name is sea or outside the coverage of the dataset.
fn defined_tiles(root: &Path) -> Result<BTreeSet<(u32, u32)>, String> {
    let registry = obc_data::sources::Registry::load(root)?;
    let datasets = obc_data::datasets::load(root, &registry)?;
    let dataset =
        datasets.iter().find(|dataset| dataset.id == MODIS).ok_or_else(|| format!("no data/archive/{MODIS}.toml"))?;
    let names = dataset.stac.iter().flat_map(|stac| &stac.names).chain(dataset.cmr.iter().flat_map(|cmr| &cmr.names));
    names
        .map(|name| {
            let tile = name.trim_matches('.').strip_prefix('h').and_then(|tile| tile.split_once('v'));
            tile.and_then(|(h, v)| Some((h.parse().ok()?, v.parse().ok()?)))
                .ok_or_else(|| format!("data/archive/{MODIS}.toml: `{name}` is not a MODIS tile such as `.h18v04.`"))
        })
        .collect()
}

/// The tile `(h, v)` and the day, from 1970-01-01, of a MODIS file at `path`: a name that
/// `MODIS_FILE` of `tools/planner_snow.py` takes, such as
/// `MOD10A1.A2016245.h18v04.061.2021338033043_NDSI_Snow_Cover.tif` or `MYD10A1.A2025181.h18v04.061.2025183090331.hdf`.
fn modis_file(path: &str) -> Option<((u32, u32), i64)> {
    let name = path.rsplit('/').next()?;
    let [product, day, tile, collection, made, kind] = name.split('.').collect::<Vec<_>>()[..] else { return None };
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    let made = match kind {
        "tif" => made.strip_suffix("_NDSI_Snow_Cover")?,
        "hdf" => made,
        _ => return None,
    };
    let day = day.strip_prefix('A').filter(|day| day.len() == 7 && digits(day))?;
    let (h, v) = tile.strip_prefix('h')?.split_once('v')?;
    if !matches!(product, "MOD10A1" | "MYD10A1") || collection != "061" || !digits(made) || h.len() != 2 || v.len() != 2
    {
        return None;
    }
    let first = obc_data::date::parse(&format!("{}-01-01", &day[..4]))?;
    Some(((h.parse().ok()?, v.parse().ok()?), first + day[4..].parse::<i64>().ok()? - 1))
}

/// Refuse a season of `seasons` when a MODIS tile of `block` lacks files of both platforms on
/// more than [`MAX_GAP_DAYS`] days in a row: a tile of `files`, or a tile of `defined` that the block
/// meets, which then lacks every day when the dataset holds none of its files.
fn modis_days(
    block: (u32, u32),
    files: &[Named],
    defined: &BTreeSet<(u32, u32)>,
    seasons: &[u16],
) -> Result<(), String> {
    let (columns, rows) = modis_tiles(block);
    let mut days: BTreeMap<(u32, u32), BTreeSet<i64>> = BTreeMap::new();
    for &(h, v) in defined.iter().filter(|(h, v)| columns.contains(h) && rows.contains(v)) {
        days.entry((h, v)).or_default();
    }
    for &(_, (tile, day)) in files {
        days.entry(tile).or_default().insert(day);
    }
    let start = |season: u16| obc_data::date::parse(&format!("{season}-09-01")).expect("a date");
    for (&(h, v), held) in &days {
        for &season in seasons {
            let (first, end) = (start(season), start(season + 1));
            let (mut run, mut gap, mut from) = (0, 0, first);
            for day in first..end {
                run = if held.contains(&day) { 0 } else { run + 1 };
                if run > gap {
                    (gap, from) = (run, day + 1 - run);
                }
            }
            if gap > MAX_GAP_DAYS {
                return Err(format!(
                    "block {}-{}: {MODIS} has no file of the MODIS tile h{h:02}v{v:02} for {gap} days from {} in season {season}. Fix: complete the dataset with obc data archive get {MODIS}",
                    block.0,
                    block.1,
                    obc_data::date::format(from)
                ));
            }
        }
    }
    Ok(())
}

/// The columns h and the rows v of the MODIS tiles that block `(x, y)` meets with two MODIS pixels
/// of margin, as far as the grid of the program reaches. A sinusoidal x is widest at the latitude
/// nearest the equator and narrowest at the farthest, so its extremes are at the west and east
/// edges, at the north, the south or the equator.
fn modis_tiles(block: (u32, u32)) -> (RangeInclusive<u32>, RangeInclusive<u32>) {
    let margin = 2.0 * MODIS_TILE_M / 2400.0;
    let [west, south, east, north] = block_bounds(block);
    let lats = [south, north, 0f64.clamp(south, north)];
    let xs =
        [west, east].into_iter().flat_map(|lon| lats.map(|lat| MODIS_R * lon.to_radians() * lat.to_radians().cos()));
    let (left, right) = xs.fold((f64::MAX, f64::MIN), |(low, high), x| (low.min(x), high.max(x)));
    let tile = |metres: f64, last: u32| ((metres / MODIS_TILE_M).floor().max(0.0) as u32).min(last);
    let columns = tile(left - margin - MODIS_X0, 35)..=tile(right + margin - MODIS_X0, 35);
    let rows = tile(MODIS_Y0 - MODIS_R * north.to_radians() - margin, 17)
        ..=tile(MODIS_Y0 - MODIS_R * south.to_radians() + margin, 17);
    (columns, rows)
}

/// The `hansen-gfc` fetch of `tiles`, at the `start` of its row in `sources`.
fn canopy(sources: &[Source], tiles: BTreeSet<String>) -> Result<Wanted, String> {
    let row = sources.iter().find(|source| source.id == CANOPY);
    let version = row.and_then(|row| row.start.clone()).ok_or_else(|| format!("`{CANOPY}` has no `start`"))?;
    let params = tiles.into_iter().map(|tile| ("tile".into(), tile)).collect();
    Ok(Wanted { source: CANOPY.into(), version: Some(version), params })
}

/// The 10° Hansen GFC tiles that `bounds` overlaps with two MODIS pixels, 2/240° of latitude, of
/// margin: the MODIS grid of a block reaches up to two pixels past it. Named by their north-west
/// corner.
fn canopy_tiles([west, south, east, north]: [f64; 4]) -> Vec<String> {
    let pad = 2.0 / 240.0;
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
    use obc_data::date;
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

        [[source]]
        id = "modis-snow"
        kind = "data"
        licence = "CC-BY-4.0"
        attribution = "Test"
        fetch = { kind = "capture", url = "https://example.org/search" }
        version = "date"
        keep = "archive"
        redistribute = true
    "#;

    /// The tiles of the dataset `modis-snow`: the Alps, and the tile east of them.
    const DATASET: &str = r#"name = "MODIS"
sources = ["modis-snow"]
coverage = "Test"
stac = [{ search = "https://example.org/search", sas = "https://example.org/sas", collection = "c", asset = "a", bbox = [0.0, 40.0, 20.0, 50.0], days = ["2016-09-01", "2026-08-31"], names = [".h18v04.", ".h19v04."] }]
"#;

    /// The tool as a script: each file of a block holds its season and block, and its mark the
    /// same; `made` gets the block, its seasons and the count of the MODIS files of its list.
    const TOOL: &str = r#"block=$3; shift 3
while [ $# -gt 0 ]; do case "$1" in --seasons) seasons=$2; shift 2;; --modis) modis=$2; shift 2;; --canopy|--slots) shift 2;; --out) out=$2; shift 2;; esac; done
x=${block%-*}; y=${block#*-}
for season in $(echo "$seasons" | tr , ' '); do mkdir -p "$out/$season/$x" && echo "$season $block" > "$out/$season/$x/$y.pmtiles" && echo "$season $block" > "$out/$block.$season.sha256"; done
echo "$block $seasons $(wc -l < "$modis")" >> made
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
    fn the_modis_tiles_of_a_block_are_those_of_its_edges_with_two_pixels_of_margin() {
        let margin = 2.0 * MODIS_TILE_M / 2400.0;
        let tile = |metres: f64| ((metres / MODIS_TILE_M).floor().max(0.0) as u32).min(35);
        // The blocks of Europe, each edge in 1000 steps: a sinusoidal x of an edge meridian varies
        // with the latitude.
        for x in 24..=41 {
            for y in 9..=28 {
                let [west, south, east, north] = block_bounds((x, y));
                let (mut left, mut right) = (f64::MAX, f64::MIN);
                for step in 0..=1000 {
                    let lat = south + (north - south) * f64::from(step) / 1000.0;
                    for lon in [west, east] {
                        let metres = MODIS_R * lon.to_radians() * lat.to_radians().cos();
                        (left, right) = (left.min(metres), right.max(metres));
                    }
                }
                let columns = tile(left - margin - MODIS_X0)..=tile(right + margin - MODIS_X0);
                assert_eq!(modis_tiles((x, y)).0, columns, "block {x}, {y}");
            }
        }
        // The west edge of block 35, 20, on the Polish coast, reaches h18 at its north corner only.
        assert_eq!(modis_tiles((35, 20)), (18..=19, 3..=3));
    }

    #[test]
    fn a_modis_file_is_a_cog_or_a_granule_of_mod10a1_or_myd10a1() {
        let day = date::parse("2016-09-01").unwrap();
        let cog = "mirror/x/MOD10A1.A2016245.h18v04.061.2021338033043_NDSI_Snow_Cover.tif";
        assert_eq!(modis_file(cog), Some(((18, 4), day)));
        assert_eq!(modis_file("MYD10A1.A2016245.h18v04.061.2021338033043.hdf"), Some(((18, 4), day)));
        for stray in [
            "MOD10A1.A2016245.h18v04.061.1.hdf.xml",
            "MOD10A2.A2016245.h18v04.061.1.hdf",
            "MOD10A1.A2016245.h18v04.006.1.hdf",
        ] {
            assert_eq!(modis_file(stray), None, "{stray}");
        }
    }

    #[test]
    fn a_season_whose_modis_tile_lacks_more_than_a_week_of_days_refuses() {
        let listed = |path: &str| archive::Listed {
            path: path.into(),
            bytes: 1,
            sha256: path.into(),
            modified: None,
            etag: None,
        };
        let first = date::parse("2016-09-01").unwrap();
        let file = listed("f");
        let days = |gap: i64| -> Vec<_> {
            (first..first + 365).filter(|day| !(first + 122..first + 122 + gap).contains(day)).collect()
        };
        let check = |days: &[i64], defined: &[(u32, u32)]| {
            let files: Vec<_> = days.iter().map(|&day| (&file, ((18, 4), day))).collect();
            modis_days((34, 22), &files, &defined.iter().copied().collect(), &[2016])
        };
        assert_eq!(check(&days(MAX_GAP_DAYS), &[(18, 4)]), Ok(()));
        let error = check(&days(MAX_GAP_DAYS + 1), &[(18, 4)]).unwrap_err();
        assert!(error.contains("h18v04 for 8 days from 2017-01-01 in season 2016"), "{error}");
        // A tile of the definition that the block meets, without any file, lacks the whole season;
        // a tile of the definition far away does not count.
        let error = check(&days(0), &[(18, 4), (19, 4)]).unwrap_err();
        assert!(error.contains("h19v04 for 365 days from 2016-09-01"), "{error}");
        assert_eq!(check(&days(0), &[(18, 4), (25, 4)]), Ok(()));
    }

    #[test]
    fn a_version_makes_the_seasons_of_its_blocks_that_the_version_before_lacks() {
        let temp = temp("snow");
        let root = temp.0.join("repository");
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/sources.toml"), SOURCES).unwrap();
        std::fs::write(root.join("tool.sh"), TOOL).unwrap();
        std::fs::create_dir_all(root.join("data/archive")).unwrap();
        std::fs::write(root.join("data/archive/modis-snow.toml"), DATASET).unwrap();
        // The dataset `modis-snow`: a file of each day of the MODIS tile of the Alps, h18v04, and of
        // the one east of it, and a stray file of metadata.
        let archive = temp.0.join("archive");
        let (from, end) = (date::parse("2016-09-01").unwrap(), date::parse("2026-09-01").unwrap());
        let mut listed = Vec::new();
        for day in from..end {
            let (year, start) =
                (&date::format(day)[..4], date::parse(&format!("{}-01-01", &date::format(day)[..4])).unwrap());
            for (tile, kind) in [("h18v04", ".1_NDSI_Snow_Cover.tif"), ("h19v04", ".1.hdf")] {
                listed.push(format!("MOD10A1.A{year}{:03}.{tile}.061{kind}", day - start + 1));
            }
        }
        listed.push("MOD10A1.A2016245.h18v04.061.1.hdf.xml".into());
        let listed: Vec<_> = listed
            .iter()
            .map(|name| serde_json::json!({"path": format!("mirror/example.org/{name}"), "bytes": 1, "sha256": name}))
            .collect();
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
        let days = end - from;
        assert_eq!(made, format!("33-22 {all} {days}\n34-22 {all} {}\n", 2 * days), "a block is made once");

        // A version of everything has every block.
        let planning = obc_data::prepared::Planning { store: &store, http: &http, sources: &sources };
        let everything = producer.plan(&planning, &sources[0], None, &[]).unwrap();
        assert_eq!(everything.files, Some(64 * 64 * seasons().unwrap().count() as u64));
    }
}
