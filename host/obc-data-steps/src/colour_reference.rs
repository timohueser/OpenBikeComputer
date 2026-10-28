//! The producer of `colour`: the Sentinel-2 texels of the zoom 12 tiles of each zoom 10 block of its
//! region, per quarter of the year `trailview.colour_year`, `<year>/<x>/<y>.pmtiles`
//! (`specs/obc-data.md`). A version extends the version before: it makes only the files that the
//! version before lacks. `tools/trailview/colour.py prepare` writes the file of one block.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use obc_bake::coverage::Coverage;
use obc_data::prepared::{Job, Plan, Prepared, Scope};
use obc_data::product::Wanted;
use obc_data::regions::Bbox;
use obc_data::sources::Source;
use planner_router_build::grid::tile_bounds;

use crate::device::box_poly;
use crate::forest_reference::box_tiles;
use crate::prepared_tiles::{self, outlines, planned, read_on_day, reuse, Tile, BATCH};

/// The id of the prepared source.
pub const ID: &str = "colour";
pub const FORMAT: u32 = 1;
const INPUT: &str = "sentinel-2-mosaics";
/// The zoom of a block.
const ZOOM: u32 = 10;
/// CDSE S3 allows 4 connections per user, and a program opens one.
const RUNNING: usize = 4;
/// The windows of a block at most: 265 kB/km² of the Central Swiss capture of `sentinel-2-mosaics`
/// over twice a block at the equator, 2 × 1530 km², as a block can span mosaic tiles that overlap.
const WINDOW_BYTES: u64 = 820_000_000;
/// The largest file of a block of the Central Swiss and Kerry captures, with a margin.
const FILE_BYTES: u64 = 40_000_000;

pub struct ColourReference {
    /// `tools/trailview/colour.py`, as a command in the repository root.
    pub tool: fn(&Path) -> Result<Command, String>,
}

pub const COLOUR_REFERENCE: ColourReference = ColourReference { tool };

fn tool(root: &Path) -> Result<Command, String> {
    let mut command = Command::new("uv");
    command.args(["run", "--locked", "--offline", "--no-default-groups", "--no-python-downloads"]);
    command.args(["--group", "trailview", "python", "-m", "tools.trailview.colour"]);
    command.env("UV_PYTHON", obc_data::engine::python_executable(root)?).env("UV_NO_SYNC", "0");
    Ok(command)
}

impl Prepared for ColourReference {
    fn name(&self) -> &'static str {
        ID
    }

    fn format(&self) -> u32 {
        FORMAT
    }

    fn revision(&self) -> u32 {
        1
    }

    /// `sentinel-2-mosaics`, and the polygons of the Geofabrik leaves of the region. The blocks of
    /// a region of boxes are the files at most.
    fn plan(
        &self,
        _: &obc_data::prepared::Planning,
        _: &Source,
        scope: Option<&Scope>,
        _: &[String],
    ) -> Result<Plan, String> {
        let scope = scope.ok_or_else(refused)?;
        let (polygons, outlines) = planned(scope)?;
        let mut inputs = vec![(Wanted { source: INPUT.into(), version: None, params: Vec::new() }, None).into()];
        inputs.extend(polygons);
        let held = outlines.as_deref().map(blocks_of).transpose()?;
        // The windows and the file of each program, and a batch that waits for its move into the home.
        let running = held.as_ref().map_or(RUNNING, |blocks| blocks.len().min(RUNNING)) as u64;
        let work_bytes = running * (WINDOW_BYTES + FILE_BYTES) + BATCH as u64 * FILE_BYTES;
        Ok(Plan { inputs, files: held.map(|blocks| blocks.len() as u64), work_bytes })
    }

    /// The file of each block of the region that the version before lacks, made once and moved into
    /// the home with the batch of its block; every file of the version before stays.
    fn prepare(&self, job: &mut Job) -> Result<(), String> {
        let previous = job.extend(FORMAT)?;
        let held: BTreeSet<&str> = previous.iter().flat_map(|manifest| &manifest.files).map(|f| &f.path[..]).collect();
        let region = blocks_of(&outlines(job)?.ok_or_else(refused)?)?;
        let year = crate::planner::colour_year()?;
        let tiles: Vec<Tile> = region
            .into_iter()
            .filter(|&block| !held.contains(&path(year, block)[..]))
            .map(|(x, y)| Tile {
                name: format!("{x}-{y}"),
                files: vec![(path(year, (x, y)), format!("{x}-{y}.sha256"))],
                marks: Vec::new(),
            })
            .collect();
        let digests = prepared_tiles::make(job, &tiles, RUNNING, |job, name| {
            let mut command = (self.tool)(job.root)?;
            command.args(["prepare", "--block", name, "--year", &year.to_string(), "--work"]);
            command.arg(job.state.join("windows").join(year.to_string()).join(name)).arg("--out").arg(&job.work);
            Ok(command)
        })?;
        reuse(job, previous.as_ref(), |path| !digests.contains_key(path))?;
        if !digests.is_empty() {
            read_on_day(job, INPUT, digests)?;
        }
        Ok(())
    }
}

fn refused() -> String {
    format!("{ID} has a version of a region only. Fix: name --region or --box")
}

/// The blocks whose inside meets one of `outlines`, as `(x, y)` of zoom 10.
pub(crate) fn blocks_of(outlines: &[Coverage]) -> Result<BTreeSet<(u32, u32)>, String> {
    let degrees = |udeg: i64| udeg as f64 / 1e6;
    let mut found = BTreeSet::new();
    for outline in outlines {
        let (west, south, east, north) = outline.bbox();
        let bbox = Bbox { west: degrees(west), south: degrees(south), east: degrees(east), north: degrees(north) };
        for (x, y) in box_tiles(&bbox, ZOOM) {
            let [west, south, east, north] = tile_bounds(ZOOM, x, y);
            let block = Coverage::parse_poly(&box_poly(&Bbox { west, south, east, north }))?;
            if !found.contains(&(x, y)) && outline.intersection(&block)?.is_some() {
                found.insert((x, y));
            }
        }
    }
    Ok(found)
}

/// The files of `blocks` of `year`.
pub fn files(blocks: &BTreeSet<(u32, u32)>, year: u16) -> BTreeSet<String> {
    blocks.iter().map(|&block| path(year, block)).collect()
}

/// The path of the file of block `(x, y)` of `year`.
fn path(year: u16, (x, y): (u32, u32)) -> String {
    format!("{year}/{x}/{y}.pmtiles")
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
        id = "colour"
        kind = "data"
        licence = "CC-BY-4.0"
        attribution = "Test"
        fetch = { kind = "prepared" }
        version = "date"
        keep = "prepared"
        redistribute = true
    "#;

    /// The tool as a script: the file of a block holds its block, and so does its mark; block 537-360
    /// has no data. `running` holds the programs that run, and `most` how many ran at once.
    const TOOL: &str = r#"block=$3; year=$5; out=$9
x=${block%-*}; y=${block#*-}
mkdir -p running "$out/$year/$x" && touch "running/$block" && ls running | wc -l >> most && sleep 0.3
if [ "$block" = 537-360 ]; then : > "$out/$year/$x/$y.pmtiles"; else echo "$block" > "$out/$year/$x/$y.pmtiles"; fi
echo "$block" > "$out/$block.sha256"
echo "$block" >> made; rm "running/$block"
"#;

    fn script(_: &Path) -> Result<Command, String> {
        let mut command = Command::new("sh");
        command.arg("tool.sh");
        Ok(command)
    }

    fn scope(id: &str, west: f64, east: f64) -> Scope {
        boxed(id, [west, 46.9, east, 47.0])
    }

    fn boxed(id: &str, [west, south, east, north]: [f64; 4]) -> Scope {
        let text = format!("name = \"{id}\"\nkind = \"box\"\nbox = [{west}, {south}, {east}, {north}]\n");
        Scope { region: id.into(), definitions: [(id.into(), text)].into() }
    }

    #[test]
    fn a_version_makes_the_blocks_of_its_region_that_the_version_before_lacks() {
        let temp = temp("colour");
        let root = temp.0.join("repository");
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/sources.toml"), SOURCES).unwrap();
        std::fs::write(root.join("tool.sh"), TOOL).unwrap();
        let (store, archive) = (Store::at(temp.0.join("store")), temp.0.join("archive"));
        let sources = Registry::load(&root).unwrap().sources;
        let (producer, http) = (ColourReference { tool: script }, Http::new());
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
        let year = crate::planner::colour_year().unwrap();
        let of = |blocks: &[(u32, u32)]| files(&blocks.iter().copied().collect(), year);
        let paths = |manifest: &Manifest| manifest.files.iter().map(|file| file.path.clone()).collect::<BTreeSet<_>>();

        // Block 534 of zoom 10 ends at 8.086 °E.
        let first = version("2026-10-01", scope("west", 7.8, 7.9));
        assert_eq!(paths(&first), of(&[(534, 360)]));
        let second = version("2026-10-02", scope("east", 8.0, 8.1));
        assert_eq!(paths(&second), of(&[(534, 360), (535, 360)]));
        assert_eq!(second.reused.keys().cloned().collect::<BTreeSet<_>>(), of(&[(534, 360)]));
        assert_eq!(second.extends, [first.scope]);
        assert_eq!(std::fs::read_to_string(root.join("made")).unwrap(), "534-360\n535-360\n", "a block is made once");

        // Six blocks, at most four programs at once; a block without data keeps its empty file.
        let third = version("2026-10-03", boxed("six", [8.5, 46.6, 9.4, 47.0]));
        let most = std::fs::read_to_string(root.join("most")).unwrap();
        let most = most.lines().map(|count| count.trim().parse::<usize>().unwrap()).max().unwrap();
        assert!((2..=RUNNING).contains(&most), "{most} programs at once");
        let empty = third.files.iter().find(|file| file.path == path(year, (537, 360))).unwrap();
        assert_eq!(empty.size, 0);
        assert_eq!(paths(&third).len(), 2 + 6);

        let planning = obc_data::prepared::Planning { store: &store, http: &http, sources: &sources };
        let refused = producer.plan(&planning, &sources[0], None, &[]).err().unwrap();
        assert!(refused.contains("a version of a region only"), "{refused}");
        let planned = producer.plan(&planning, &sources[0], Some(&scope("east", 8.0, 8.1)), &[]).unwrap();
        assert_eq!(planned.files, Some(2));
    }
}
