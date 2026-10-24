//! The tiles of a prepared source that a program of its producer makes, one tile per run of the
//! program: `terrain` and `forest`.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::Path;
use std::process::Command;

use obc_data::prepared::{Job, Program};

/// The tiles of one upload.
const BATCH: usize = 64;
/// The programs that run at once. Eight, chosen by the owner: a program waits mostly on its
/// downloads, and one tile needs little memory.
pub(crate) const RUNNING: usize = 8;

/// A tile: its name, and its files in the version, each with the mark in `state/tiles/` that holds
/// its pixel digest; `marks` are other marks that the program writes before those.
pub struct Tile {
    pub name: String,
    pub files: Vec<(String, String)>,
    pub marks: Vec<String>,
}

/// Make each tile that the version lacks, [`RUNNING`] at a time, and upload its files in batches.
/// `command(job, name)` is the program that writes the files of tile `name` into `job.work`, then
/// their marks, the mark of the last file last; or the empty `<name>.none` for a tile without data.
/// Each line of its output, and its error, starts with `tile k/K <name>`: tile k of the K tiles of
/// the version. `state/tiles/` keeps the marks, so a resume makes no tile again that it has. Gives
/// the pixel digest of each file of the version by path.
pub fn make(
    job: &mut Job,
    tiles: &[Tile],
    command: impl Fn(&Job, &str) -> Result<Command, String>,
) -> Result<BTreeMap<String, String>, String> {
    let marks = job.state.join("tiles");
    fs::create_dir_all(&marks).map_err(|e| format!("{}: {e}", marks.display()))?;
    // The run shows each tile that a program makes as a step of the source, with its progress.
    let todo = tiles.iter().filter(|tile| !done(job, &marks, tile) && !waiting(job, &marks, tile));
    let steps: Vec<String> = todo.map(|tile| step(job, tile)).collect();
    job.plan_steps(&steps)?;
    let mut batch = Vec::new();
    // Oldest first. A return drops the programs that still run, which stops them.
    let mut running: VecDeque<(&Tile, Program)> = VecDeque::new();
    for (k, tile) in tiles.iter().enumerate() {
        if done(job, &marks, tile) {
            continue;
        }
        if waiting(job, &marks, tile) {
            add(job, tile, &mut batch)?;
            continue;
        }
        if running.len() == RUNNING {
            let (made, program) = running.pop_front().expect("a program runs");
            ended(job, &marks, made, program, &mut batch)?;
        }
        if let Some(at) = running.iter_mut().position(|(_, program)| program.failed()) {
            let (failed, program) = running.remove(at).expect("a program runs");
            ended(job, &marks, failed, program, &mut batch)?;
        }
        let label = format!("tile {}/{} {}", k + 1, tiles.len(), tile.name);
        eprintln!("{label}");
        // The programs are Python, which holds the lines that it writes to a pipe until it ends.
        let mut program = command(job, &tile.name)?;
        program.env("PYTHONUNBUFFERED", "1");
        let program = job.start(&mut program, Some(&label))?;
        job.step_started(&step(job, tile))?;
        running.push_back((tile, program));
    }
    while let Some((made, program)) = running.pop_front() {
        ended(job, &marks, made, program, &mut batch)?;
    }
    job.finish(&batch)?;
    let mut digests = BTreeMap::new();
    for (path, mark) in tiles.iter().flat_map(|tile| &tile.files).filter(|(path, _)| job.done(path)) {
        let digest = fs::read_to_string(marks.join(mark)).map_err(|e| format!("{mark}: {e}"))?;
        digests.insert(path.clone(), digest.trim().to_string());
    }
    Ok(digests)
}

/// Whether the target has the files of `tile`, or the tile has no data.
fn done(job: &Job, marks: &Path, tile: &Tile) -> bool {
    tile.files.iter().all(|(path, _)| job.done(path)) || marks.join(format!("{}.none", tile.name)).exists()
}

/// Whether a run made the files of `tile` and stopped before their upload.
fn waiting(job: &Job, marks: &Path, tile: &Tile) -> bool {
    tile.files.iter().all(|(path, mark)| marks.join(mark).exists() && job.work.join(path).exists())
}

/// The step of `tile` in the run: `terrain/4804-4215`.
fn step(job: &Job, tile: &Tile) -> String {
    format!("{}/{}", job.id, tile.name)
}

/// Wait for the program of `tile` and record its step: on success what it used and the bytes of
/// its files, then [`keep`] them.
fn ended(job: &mut Job, marks: &Path, tile: &Tile, program: Program, batch: &mut Vec<String>) -> Result<(), String> {
    match program.wait_usage() {
        Ok(usage) => {
            let bytes = tile
                .files
                .iter()
                .filter_map(|(path, _)| fs::metadata(job.work.join(path)).ok())
                .map(|file| file.len())
                .sum();
            job.step_finished(&step(job, tile), &usage, bytes)?;
            keep(job, marks, tile, batch)
        }
        Err(e) => {
            job.step_failed(&step(job, tile), &e)?;
            Err(e)
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

/// Add the files of `tile` to the batch, and upload a full batch.
fn add(job: &mut Job, tile: &Tile, batch: &mut Vec<String>) -> Result<(), String> {
    batch.extend(tile.files.iter().map(|(path, _)| path.clone()));
    if batch.len() >= BATCH {
        job.finish(&std::mem::take(batch))?;
    }
    Ok(())
}
