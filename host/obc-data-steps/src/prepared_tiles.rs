//! The tiles of a prepared source that a program of its producer makes, one tile per run of the
//! program: `terrain-reference/<key>` and `forest-reference/<key>`.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::Path;
use std::process::Command;

use obc_data::prepared::{Job, Program};

/// The tiles of one upload.
const BATCH: usize = 64;
/// The programs that run at once: while one pools its tile, the next downloads.
const RUNNING: usize = 2;

/// A tile: its name, and its files in the version, each with the mark in `state/tiles/` that holds
/// its pixel digest.
pub struct Tile {
    pub name: String,
    pub files: Vec<(String, String)>,
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
    let mut batch = Vec::new();
    // Oldest first. A return drops the programs that still run, which stops them.
    let mut running: VecDeque<(&Tile, Program)> = VecDeque::new();
    for (k, tile) in tiles.iter().enumerate() {
        let none = format!("{}.none", tile.name);
        if tile.files.iter().all(|(path, _)| job.done(path)) || marks.join(&none).exists() {
            continue;
        }
        if tile.files.iter().all(|(path, mark)| marks.join(mark).exists() && job.work.join(path).exists()) {
            add(job, tile, &mut batch)?;
            continue;
        }
        if running.len() == RUNNING {
            let (made, program) = running.pop_front().expect("a program runs");
            program.wait()?;
            keep(job, &marks, made, &mut batch)?;
        }
        if let Some(at) = running.iter_mut().position(|(_, program)| program.failed()) {
            running.remove(at).expect("a program runs").1.wait()?;
        }
        let label = format!("tile {}/{} {}", k + 1, tiles.len(), tile.name);
        eprintln!("{label}");
        // The programs are Python, which holds the lines that it writes to a pipe until it ends.
        let mut program = command(job, &tile.name)?;
        program.env("PYTHONUNBUFFERED", "1");
        running.push_back((tile, job.start(&mut program, Some(&label))?));
    }
    while let Some((made, program)) = running.pop_front() {
        program.wait()?;
        keep(job, &marks, made, &mut batch)?;
    }
    job.finish(&batch)?;
    let mut digests = BTreeMap::new();
    for (path, mark) in tiles.iter().flat_map(|tile| &tile.files).filter(|(path, _)| job.done(path)) {
        let digest = fs::read_to_string(marks.join(mark)).map_err(|e| format!("{mark}: {e}"))?;
        digests.insert(path.clone(), digest.trim().to_string());
    }
    Ok(digests)
}

/// Move the marks that the program of `tile` wrote into `marks`, and add its files to the batch.
fn keep(job: &mut Job, marks: &Path, tile: &Tile, batch: &mut Vec<String>) -> Result<(), String> {
    let none = format!("{}.none", tile.name);
    let written = tile.files.iter().map(|(_, mark)| mark).chain([&none]);
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
