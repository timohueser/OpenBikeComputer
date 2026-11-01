//! The producer of `climate`: the daily values of ERA5-Land of each block of 96 × 64 cells of its
//! region, `<year>/<x>/<y>.gz` for the ten years that `planner/climate` reads, and the geopotential
//! `orography/<x>/<y>.gz` (`specs/obc-data.md`). A version extends the version before: it makes
//! only the files that the version before lacks, as final data of a year does not change.
//! `tools/planner_climate.py prepare` writes the files of one block.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use obc_bake::coverage::Coverage;
use obc_data::prepared::{Job, Plan, Prepared, Scope};
use obc_data::product::Wanted;
use obc_data::regions::Bbox;
use obc_data::sources::Source;

use crate::device::box_poly;
use crate::prepared_tiles::{self, outlines, planned, read_on_day, reuse, Tile, BATCH, RUNNING};

/// The id of the prepared source.
pub const ID: &str = "climate";
pub const FORMAT: u32 = 1;
/// The years that `planner/climate` reads: `YEARS` of `tools/planner_climate.py`.
pub(crate) const YEARS: u16 = 10;
const INPUT: &str = "era5-land";
/// The cells of a block, 16 × 12 chunks of the ARCO stores, and of the ERA5-Land grid.
const ROWS: u32 = 64;
const COLUMNS: u32 = 96;
const GRID_ROWS: u32 = 1801;
const GRID_COLUMNS: u32 = 3600;
/// How far a block reaches past its cells, two cells: the bake of a box reads the cells that
/// overlap it and two more on each side for the lapse rates.
const MARGIN: f64 = 0.2;
/// The nearest that a region comes to the antimeridian: there the bake would read cells on both sides.
const ANTIMERIDIAN: f64 = 0.3;
/// The largest chunk of the DACH capture of `era5-land` in the store.
const CHUNK_BYTES: u64 = 2_910_000;
/// A file of a block-year of land cells, from cached ERA5-Land chunks of Central Switzerland.
const FILE_BYTES: u64 = 46_000_000;
/// The chunk cache of a program at most: two time chunks of the 4 variables of the 16 × 12 chunks.
const CACHE_BYTES: u64 = 2 * 4 * 16 * 12 * CHUNK_BYTES;

pub struct ClimateReference {
    /// `tools/planner_climate.py`, as a command in the repository root.
    pub tool: fn(&Path) -> Result<Command, String>,
}

pub const CLIMATE_REFERENCE: ClimateReference = ClimateReference { tool };

fn tool(root: &Path) -> Result<Command, String> {
    let mut command = Command::new("uv");
    command.args(["run", "--locked", "--offline", "--no-default-groups", "--no-python-downloads"]);
    command.args(["--group", "planner-climate", "python", "-m", "tools.planner_climate"]);
    command.env("UV_PYTHON", obc_data::engine::python_executable(root)?).env("UV_NO_SYNC", "0");
    Ok(command)
}

impl Prepared for ClimateReference {
    fn name(&self) -> &'static str {
        ID
    }

    fn format(&self) -> u32 {
        FORMAT
    }

    fn revision(&self) -> u32 {
        1
    }

    fn reads(&self, source: &Source) -> bool {
        source.id == INPUT
    }

    /// `era5-land`, and the polygons of the Geofabrik leaves of the region. The files of the blocks
    /// of a region of boxes are the files at most.
    fn plan(
        &self,
        _: &obc_data::prepared::Planning,
        _: &Source,
        scope: Option<&Scope>,
        _: &[String],
    ) -> Result<Plan, String> {
        let mut inputs = vec![(Wanted { source: INPUT.into(), version: None, params: Vec::new() }, None).into()];
        let held = match scope {
            None => Some(every()),
            Some(scope) => {
                let (polygons, outlines) = planned(scope)?;
                inputs.extend(polygons);
                outlines.as_deref().map(blocks_of).transpose()?
            }
        };
        // The chunk caches and the files of the programs that run at once, and a batch that waits
        // for its move into the home.
        let running = held.as_ref().map_or(RUNNING, |blocks| blocks.len().min(RUNNING)) as u64;
        let year_files = 1 + u64::from(YEARS);
        let work_bytes = running * (CACHE_BYTES + year_files * FILE_BYTES) + BATCH as u64 * FILE_BYTES;
        Ok(Plan { inputs, files: held.map(|blocks| blocks.len() as u64 * year_files), work_bytes })
    }

    /// The files of each block of the region that the version before lacks, made once and moved into
    /// the home with the batch of their block; every file of the version before stays.
    fn prepare(&self, job: &mut Job) -> Result<(), String> {
        let previous = job.extend(FORMAT)?;
        let held: BTreeSet<&str> = previous.iter().flat_map(|manifest| &manifest.files).map(|f| &f.path[..]).collect();
        let region = match outlines(job)? {
            Some(outlines) => blocks_of(&outlines)?,
            None => every(),
        };
        let first_year = crate::planner::climate_first_year()?;
        let mut tiles = Vec::new();
        // The arguments of the program of each block.
        let mut args: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (x, y) in region {
            let name = format!("{x}-{y}");
            let mut files = Vec::new();
            let (mut years, mut orography) = (Vec::new(), false);
            if !held.contains(&path("orography", (x, y))[..]) {
                files.push((path("orography", (x, y)), format!("{name}.orography.sha256")));
                orography = true;
            }
            for year in first_year..first_year + YEARS {
                if !held.contains(&path(year, (x, y))[..]) {
                    files.push((path(year, (x, y)), format!("{name}.{year}.sha256")));
                    years.push(year.to_string());
                }
            }
            if files.is_empty() {
                continue;
            }
            let mut named = vec!["--block".to_string(), name.clone()];
            if !years.is_empty() {
                named.extend(["--years".to_string(), years.join(",")]);
            }
            if orography {
                // The programs share one download of the orography file.
                named.extend(["--orography".to_string(), job.state.join("orography").display().to_string()]);
            }
            args.insert(name.clone(), named);
            tiles.push(Tile { name, files, marks: Vec::new() });
        }
        let digests = prepared_tiles::make(job, &tiles, RUNNING, |job, name| {
            let mut command = (self.tool)(job.root)?;
            command.arg("prepare").args(&args[name]);
            command.arg("--work").arg(job.state.join("raw").join(name)).arg("--out").arg(&job.work);
            Ok(command)
        })?;
        reuse(job, previous.as_ref(), |path| !digests.contains_key(path))?;
        if !digests.is_empty() {
            read_on_day(job, INPUT, digests)?;
        }
        Ok(())
    }
}

/// The blocks that `outline` meets after each block grows by [`MARGIN`] on each side, as `(x, y)`:
/// the blocks of the cells that overlap the outline and of the cells two cells around them.
pub(crate) fn blocks(outline: &Coverage) -> Result<BTreeSet<(u32, u32)>, String> {
    let (west, south, east, north) = outline.bbox();
    let degrees = |udeg: i64| udeg as f64 / 1e6;
    if degrees(west) < -180.0 + ANTIMERIDIAN || degrees(east) > 180.0 - ANTIMERIDIAN {
        return Err(format!("{ID} has no region within {ANTIMERIDIAN}° of the antimeridian"));
    }
    // The block of a longitude and of a latitude, each in the grid: a block x holds the cells whose
    // Zarr column 10 (lon + 180) − 1 is 96x to 96x + 95, and a block y those whose Zarr row
    // 10 (lat + 90) is 64y to 64y + 63.
    let x = |lon: f64| {
        ((10.0 * (lon + 180.0) - 0.5) / f64::from(COLUMNS))
            .floor()
            .clamp(0.0, f64::from(GRID_COLUMNS.div_ceil(COLUMNS) - 1)) as u32
    };
    let y = |lat: f64| {
        ((10.0 * (lat + 90.0) + 0.5) / f64::from(ROWS)).floor().clamp(0.0, f64::from(GRID_ROWS.div_ceil(ROWS) - 1))
            as u32
    };
    let mut found = BTreeSet::new();
    for bx in x(degrees(west) - MARGIN)..=x(degrees(east) + MARGIN) {
        for by in y(degrees(south) - MARGIN)..=y(degrees(north) + MARGIN) {
            let last_column = (COLUMNS * (bx + 1)).min(GRID_COLUMNS) - 1;
            let last_row = (ROWS * (by + 1)).min(GRID_ROWS) - 1;
            let grown = Bbox {
                west: (f64::from(COLUMNS * bx) + 0.5) / 10.0 - 180.0 - MARGIN,
                south: (f64::from(ROWS * by) - 0.5) / 10.0 - 90.0 - MARGIN,
                east: (f64::from(last_column) + 1.5) / 10.0 - 180.0 + MARGIN,
                north: (f64::from(last_row) + 0.5) / 10.0 - 90.0 + MARGIN,
            };
            if outline.intersection(&Coverage::parse_poly(&box_poly(&grown))?)?.is_some() {
                found.insert((bx, by));
            }
        }
    }
    Ok(found)
}

/// The blocks of each of `outlines`.
fn blocks_of(outlines: &[Coverage]) -> Result<BTreeSet<(u32, u32)>, String> {
    let mut found = BTreeSet::new();
    for outline in outlines {
        found.extend(blocks(outline)?);
    }
    Ok(found)
}

/// The files of `blocks` that the ten years from `first_year` need: the orography and each year.
pub fn files(blocks: &BTreeSet<(u32, u32)>, first_year: u16) -> BTreeSet<String> {
    let years = first_year..first_year + YEARS;
    let paths = |block: (u32, u32)| {
        std::iter::once(path("orography", block)).chain(years.clone().map(move |year| path(year, block)))
    };
    blocks.iter().copied().flat_map(paths).collect()
}

/// The path of the file `name`, `orography` or a year, of block `(x, y)`.
fn path(name: impl std::fmt::Display, (x, y): (u32, u32)) -> String {
    format!("{name}/{x}/{y}.gz")
}

/// Every block, for a version of everything.
fn every() -> BTreeSet<(u32, u32)> {
    let (columns, rows) = (GRID_COLUMNS.div_ceil(COLUMNS), GRID_ROWS.div_ceil(ROWS));
    (0..columns).flat_map(|x| (0..rows).map(move |y| (x, y))).collect()
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
        id = "climate"
        kind = "data"
        licence = "CC-BY-4.0"
        attribution = "Test"
        fetch = { kind = "prepared" }
        version = "date"
        keep = "prepared"
        redistribute = true
    "#;

    /// The tool as a script: each file of a block holds its path, and its mark its year and block.
    const TOOL: &str = r#"block=$3; shift 3; years=""; orography=""
while [ $# -gt 0 ]; do case "$1" in --years) years=$2; shift 2;; --orography) orography=$2; shift 2;; --work) shift 2;; --out) out=$2; shift 2;; esac; done
x=${block%-*}; y=${block#*-}
[ -n "$orography" ] && mkdir -p "$out/orography/$x" && echo "o $block" > "$out/orography/$x/$y.gz" && echo "o $block" > "$out/$block.orography.sha256"
for year in $(echo "$years" | tr , ' '); do mkdir -p "$out/$year/$x" && echo "$year $block" > "$out/$year/$x/$y.gz" && echo "$year $block" > "$out/$block.$year.sha256"; done
echo "$block" >> made
"#;

    fn script(_: &Path) -> Result<Command, String> {
        let mut command = Command::new("sh");
        command.arg("tool.sh");
        Ok(command)
    }

    fn scope(id: &str, west: f64, east: f64) -> Scope {
        let text = format!("name = \"{id}\"\nkind = \"box\"\nbox = [{west}, 47.0, {east}, 47.1]\n");
        Scope { region: id.into(), definitions: [(id.into(), text)].into() }
    }

    #[test]
    fn a_version_makes_the_files_of_its_blocks_that_the_version_before_lacks() {
        let temp = temp("climate");
        let root = temp.0.join("repository");
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/sources.toml"), SOURCES).unwrap();
        std::fs::write(root.join("tool.sh"), TOOL).unwrap();
        let (store, archive) = (Store::at(temp.0.join("store")), temp.0.join("archive"));
        let sources = Registry::load(&root).unwrap().sources;
        let (producer, http) = (ClimateReference { tool: script }, Http::new());
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
                publish_text: None,
                licence: None,
                attribution: None,
                machine: "test".into(),
                time: format!("{version}T00:00:00Z"),
                cost: obc_data::prepared::Cost { wall_ms: 0, peak_rss_bytes: None },
            };
            obc_data::prepared::complete(&obc_data::prepared::folder(&archive, ID), &manifest).unwrap();
            manifest
        };
        let first_year = crate::planner::climate_first_year().unwrap();
        let of = |blocks: &[(u32, u32)]| files(&blocks.iter().copied().collect(), first_year);
        let paths = |manifest: &Manifest| manifest.files.iter().map(|file| file.path.clone()).collect::<BTreeSet<_>>();

        // 8.0 °E is in block 19, whose cells end at 12.05 °E.
        let first = version("2026-10-01", scope("west", 8.0, 8.1));
        assert_eq!(paths(&first), of(&[(19, 21)]));
        assert_eq!(paths(&first).len(), 1 + usize::from(YEARS));

        // A box 0.05° from the edge of block 19 also reads block 20 for its lapse rates.
        let second = version("2026-10-02", scope("east", 11.9, 12.0));
        assert_eq!(paths(&second), of(&[(19, 21), (20, 21)]));
        assert_eq!(second.reused.keys().cloned().collect::<BTreeSet<_>>(), of(&[(19, 21)]));
        assert!(second.reused.values().all(|version| version == "2026-10-01"));
        assert_eq!(second.extends, [first.scope]);
        assert_eq!(std::fs::read_to_string(root.join("made")).unwrap(), "19-21\n20-21\n", "a block is made once");

        // The bake of a box near the antimeridian would read cells on both sides of it; a version
        // of everything has every block.
        let east = Bbox { west: 179.5, south: 0.0, east: 179.8, north: 0.1 };
        assert!(blocks(&Coverage::parse_poly(&box_poly(&east)).unwrap()).is_err());
        let planning = obc_data::prepared::Planning { store: &store, http: &http, sources: &sources };
        let everything = producer.plan(&planning, &sources[0], None, &[]).unwrap();
        assert_eq!(everything.files, Some(38 * 29 * (1 + u64::from(YEARS))));
    }
}
