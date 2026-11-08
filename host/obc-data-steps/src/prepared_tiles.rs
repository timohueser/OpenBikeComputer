//! The tiles of a prepared source that a program of its producer makes, one tile per run of the
//! program: `terrain`, `forest`, `climate`, `colour` and `snow`. Also what the producers share around them: the
//! outlines of the region of a version, the files that a version keeps of the version before, and
//! the record of what a service gave on the day of a version.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use obc_bake::coverage::Coverage;
use obc_data::prepared::{day, Job, Manifest, Program, Scope};
use obc_data::product::Wanted;
use obc_data::regions::{Area, Regions};
use obc_data::settings::Settings;

use crate::device::{box_poly, POLY};

/// The tiles of one move into the home.
pub(crate) const BATCH: usize = 64;
/// The programs that run at once, unless a service limits the connections. Eight, chosen by the
/// owner: a program waits mostly on its downloads.
pub(crate) const RUNNING: usize = 8;
/// The programs that fail in a row, with none that ends well between them, that end the run at
/// once: that is a bug or no network, not one server that is down. Chosen by the owner above the
/// longest row of Europe blocks in plan order that one national model covers, about 100 (Norway,
/// on the box of its extent). Without network a run spends about 128 / 8 × 12.6 min of download
/// retries, 3.4 h, before it stops.
pub(crate) const IN_A_ROW: usize = 128;

/// A tile: its name, and its files in the version, each with the mark in `state/tiles/` that holds
/// its pixel digest; `marks` are other marks that the program writes before those. `limits` names
/// each source that its program reads with a `fetch.at_once`, and that number.
pub struct Tile {
    pub name: String,
    pub files: Vec<(String, String)>,
    pub marks: Vec<String>,
    pub limits: Vec<(String, usize)>,
}

/// Make each tile that the version lacks, `running_at_most` at a time, and move its files into the
/// home in batches. A free slot takes the first tile, in order, whose every limit has fewer programs
/// running than its number, so a slow source does not hold every slot. A tile whose program fails is
/// set aside, and the run goes on; once no other tile waits, each set-aside tile runs once more. A
/// tile that fails again ends the run with an error that names it, and the version stays incomplete,
/// so a resume makes only the tiles without marks. `in_a_row_at_most` failures in a row end the run
/// at once.
/// `command(job, name)` is the program that writes the files of tile `name` into `job.work`, then
/// their marks, the mark of the last file last; or the empty `<name>.none` for a tile without data.
/// Each line of its output, and its error, starts with `tile k/K <name>`: tile k of the K tiles of
/// the version. `state/tiles/` keeps the marks, so a resume makes no tile again that it has. Gives
/// the pixel digest of each file of the version by path.
pub fn make(
    job: &mut Job,
    tiles: &[Tile],
    running_at_most: usize,
    in_a_row_at_most: usize,
    command: impl Fn(&Job, &str) -> Result<Command, String>,
) -> Result<BTreeMap<String, String>, String> {
    let marks = job.state.join("tiles");
    fs::create_dir_all(&marks).map_err(|e| format!("{}: {e}", marks.display()))?;
    // The run shows each tile that a program makes as a step of the source, with its progress.
    let todo = tiles.iter().filter(|tile| !done(job, &marks, tile) && !waiting(job, &marks, tile));
    let steps: Vec<String> = todo.map(|tile| step(job, tile)).collect();
    job.plan_steps(&steps)?;
    let mut batch = Vec::new();
    let mut queue = Vec::new();
    for (k, tile) in tiles.iter().enumerate() {
        if done(job, &marks, tile) {
            continue;
        }
        if waiting(job, &marks, tile) {
            add(job, tile, &mut batch)?;
            continue;
        }
        queue.push(k);
    }
    // A return drops the programs that still run, which stops them.
    let mut running: Vec<(usize, Program)> = Vec::new();
    let (mut aside, mut again, mut in_a_row) = (Vec::new(), false, 0);
    loop {
        // Any program that ends frees its slot, so a short tile does not wait behind a long one.
        let mut at = 0;
        while at < running.len() {
            if running[at].1.ended()? {
                let (k, program) = running.remove(at);
                in_a_row += 1;
                if ended(job, &marks, &tiles[k], program, &mut batch)? {
                    in_a_row = 0;
                } else {
                    aside.push(k);
                }
                if in_a_row == in_a_row_at_most {
                    return stop(job, &batch, tiles, &aside, &format!("{in_a_row} tiles failed in a row"));
                }
            } else {
                at += 1;
            }
        }
        if queue.is_empty() && running.is_empty() {
            match (aside.is_empty(), again) {
                (true, _) => break,
                (false, true) => return stop(job, &batch, tiles, &aside, "the set-aside tiles failed again"),
                (false, false) => {
                    eprintln!("trying the {} set-aside tiles once more", aside.len());
                    (queue, again) = (std::mem::take(&mut aside), true);
                }
            }
        }
        let mut started = false;
        while running.len() < running_at_most {
            let held = |source: &str| {
                running.iter().filter(|(k, _)| tiles[*k].limits.iter().any(|(other, _)| other == source)).count()
            };
            let fits = |k: &usize| tiles[*k].limits.iter().all(|(source, most)| held(source) < *most);
            let Some(at) = queue.iter().position(fits) else { break };
            let k = queue.remove(at);
            let tile = &tiles[k];
            let label = format!("tile {}/{} {}", k + 1, tiles.len(), tile.name);
            eprintln!("{label}");
            // The programs are Python, which holds the lines that it writes to a pipe until it ends.
            let mut program = command(job, &tile.name)?;
            program.env("PYTHONUNBUFFERED", "1");
            let program = job.start(&mut program, Some(&label))?;
            job.step_started(&step(job, tile))?;
            running.push((k, program));
            started = true;
        }
        if !started {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    job.finish(&batch)?;
    let mut digests = BTreeMap::new();
    for (path, mark) in tiles.iter().flat_map(|tile| &tile.files).filter(|(path, _)| job.done(path)) {
        let digest = fs::read_to_string(marks.join(mark)).map_err(|e| format!("{mark}: {e}"))?;
        digests.insert(path.clone(), digest.trim().to_string());
    }
    Ok(digests)
}

/// Whether the home has the files of `tile`, or the tile has no data.
fn done(job: &Job, marks: &Path, tile: &Tile) -> bool {
    tile.files.iter().all(|(path, _)| job.done(path)) || marks.join(format!("{}.none", tile.name)).exists()
}

/// Whether a run made the files of `tile` and stopped before their move into the home.
fn waiting(job: &Job, marks: &Path, tile: &Tile) -> bool {
    tile.files.iter().all(|(path, mark)| marks.join(mark).exists() && job.work.join(path).exists())
}

/// The step of `tile` in the run: `terrain/4804-4215`.
fn step(job: &Job, tile: &Tile) -> String {
    format!("{}/{}", job.id, tile.name)
}

/// Move the tiles made so far into the home, and end the run with `why` and the set-aside tiles.
fn stop<T>(job: &mut Job, batch: &[String], tiles: &[Tile], aside: &[usize], why: &str) -> Result<T, String> {
    job.finish(batch)?;
    let names: Vec<&str> = aside.iter().map(|k| tiles[*k].name.as_str()).collect();
    Err(format!("{why}; {} set-aside tiles wait for a resume: {}", names.len(), names.join(", ")))
}

/// Wait for the program of `tile` and record its step: on success what it used and the bytes of
/// its files, then [`keep`] them. Whether the program ended well; a failed program is set aside.
fn ended(job: &mut Job, marks: &Path, tile: &Tile, program: Program, batch: &mut Vec<String>) -> Result<bool, String> {
    match program.wait_usage() {
        Ok(usage) => {
            let bytes = tile
                .files
                .iter()
                .filter_map(|(path, _)| fs::metadata(job.work.join(path)).ok())
                .map(|file| file.len())
                .sum();
            job.step_finished(&step(job, tile), &usage, bytes)?;
            keep(job, marks, tile, batch).map(|()| true)
        }
        Err(e) => {
            eprintln!("set aside: {e}");
            job.step_failed(&step(job, tile), &e)?;
            Ok(false)
        }
    }
}

/// Move the marks that the program of `tile` wrote into `marks`, and add its files to the batch.
fn keep(job: &mut Job, marks: &Path, tile: &Tile, batch: &mut Vec<String>) -> Result<(), String> {
    let none = format!("{}.none", tile.name);
    let written = tile.marks.iter().chain(tile.files.iter().map(|(_, mark)| mark)).chain([&none]);
    for mark in written.filter(|mark| job.work.join(mark).exists()) {
        fs::rename(job.work.join(mark), marks.join(mark)).map_err(|e| format!("{mark}: {e}"))?;
    }
    if marks.join(&none).exists() {
        return Ok(());
    }
    if tile.files.last().is_some_and(|(_, mark)| !marks.join(mark).exists()) {
        return Err(format!("the program of tile {} wrote neither its files nor `{none}`", tile.name));
    }
    add(job, tile, batch)
}

/// Add the files of `tile` to the batch, and move a full batch into the home.
fn add(job: &mut Job, tile: &Tile, batch: &mut Vec<String>) -> Result<(), String> {
    batch.extend(tile.files.iter().map(|(path, _)| path.clone()));
    if batch.len() >= BATCH {
        job.finish(&std::mem::take(batch))?;
    }
    Ok(())
}

/// The regions of `scope`, and the id of its region.
pub(crate) fn regions(scope: &Scope) -> Result<(Regions, String), String> {
    let settings =
        Settings { region: scope.region.clone(), definitions: scope.definitions.clone(), ..Settings::default() };
    Ok((settings.regions()?, scope.region.clone()))
}

/// The fetch of the `geofabrik-poly` polygon of `area`, at `version` or the newest.
pub(crate) fn poly(area: &str, version: Option<String>) -> Wanted {
    Wanted { source: POLY.into(), version, params: vec![("area".into(), area.into())] }
}

/// What a plan knows of the region of `scope` before a download: the `geofabrik-poly` fetch of each
/// area of its leaves, and the outline of each box leaf; `None` for the outlines when a leaf is an
/// area, whose polygon only the run reads.
pub(crate) fn planned(scope: &Scope) -> Result<(Vec<obc_data::prepared::Input>, Option<Vec<Coverage>>), String> {
    let (regions, id) = regions(scope)?;
    let (mut inputs, mut outlines) = (Vec::new(), Some(Vec::new()));
    for leaf in regions.leaves(&id)? {
        match &regions.get(leaf).expect("a leaf is a region").area {
            Area::Box { bbox } => {
                if let Some(outlines) = &mut outlines {
                    outlines.push(Coverage::parse_poly(&box_poly(bbox))?);
                }
            }
            Area::Geofabrik { areas } => {
                inputs.extend(areas.iter().map(|area| (poly(area, None), None).into()));
                outlines = None;
            }
            Area::Union { .. } => {}
        }
    }
    Ok((inputs, outlines))
}

/// The outline of each leaf of the region of `job`: its box, or the polygon of each of its areas;
/// `None` for a version of everything.
pub(crate) fn outlines(job: &mut Job) -> Result<Option<Vec<Coverage>>, String> {
    let Some(scope) = job.scope.clone() else { return Ok(None) };
    let (regions, id) = regions(&scope)?;
    let mut outlines = Vec::new();
    for leaf in regions.leaves(&id)? {
        match &regions.get(leaf).expect("a leaf is a region").area {
            Area::Box { bbox } => outlines.push(Coverage::parse_poly(&box_poly(bbox))?),
            Area::Geofabrik { areas } => {
                for area in areas {
                    outlines.push(outline(job, area)?);
                }
            }
            Area::Union { .. } => {}
        }
    }
    Ok(Some(outlines))
}

/// The `geofabrik-poly` polygon of `area`, at the version of the first run of the job.
pub(crate) fn outline(job: &mut Job, area: &str) -> Result<Coverage, String> {
    let path = pinned(job, area, |version| poly(area, version))?;
    Coverage::parse_poly(&fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?)
}

/// The one file of `wanted`. The first run of a version keeps the version of each `key` in
/// `state/polygons.json`, so each run of the version reads the polygons of the first.
pub(crate) fn pinned(job: &mut Job, key: &str, wanted: impl Fn(Option<String>) -> Wanted) -> Result<PathBuf, String> {
    let pins = job.state.join("polygons.json");
    let mut versions: BTreeMap<String, String> = match fs::read(&pins) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", pins.display()))?,
        Err(_) => BTreeMap::new(),
    };
    let pinned = versions.get(key).cloned();
    let (version, files) = job.fetch(&wanted(pinned.clone())).map_err(|e| match pinned {
        Some(_) => format!(
            "{e}. Fix: the version of `{key}` of the first run is gone: delete {}; the next run starts a new version",
            job.state.parent().unwrap_or(&job.state).display()
        ),
        None => e,
    })?;
    if versions.insert(key.into(), version.clone()).as_ref() != Some(&version) {
        let bytes = serde_json::to_vec(&versions).expect("versions serialize");
        obc_data::store::write_atomic(&pins, &bytes)?;
    }
    let [path] = files.values().collect::<Vec<_>>()[..] else {
        return Err(format!("a fetch of `{key}` gives {} files, not one", files.len()));
    };
    Ok(path.clone())
}

/// Keep each file of the version before for which `keep` holds, without a copy.
pub(crate) fn reuse(job: &mut Job, previous: Option<&Manifest>, keep: impl Fn(&str) -> bool) -> Result<(), String> {
    let Some(previous) = previous else { return Ok(()) };
    for path in previous.files.iter().map(|file| &file.path).filter(|path| keep(path)) {
        job.reuse(previous, path)?;
    }
    Ok(())
}

/// Record that the prepare read `files`, path to digest, of `source`: a service has no versions of
/// its own, so the day of the version names what it gave.
pub(crate) fn read_on_day(job: &mut Job, source: &str, files: BTreeMap<String, String>) -> Result<(), String> {
    let version = day(&job.version).to_string();
    job.input(source, &version, files)
}
