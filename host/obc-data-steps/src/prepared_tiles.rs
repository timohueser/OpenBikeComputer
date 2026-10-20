//! The tiles of a prepared source that a program of its producer makes one at a time:
//! `terrain-reference/<key>` and `forest-reference/<key>`.

use std::collections::BTreeMap;
use std::fs;

use obc_data::prepared::Job;

/// The tiles of one upload.
const BATCH: usize = 64;

/// A tile: its name, and its files in the version, each with the mark in `state/tiles/` that holds
/// its pixel digest.
pub struct Tile {
    pub name: String,
    pub files: Vec<(String, String)>,
}

/// Make each tile that the version lacks, and upload its files in batches. `make(job, name)` runs
/// the program that writes the files of tile `name` into `job.work`, then their marks, the mark of
/// the last file last; or the empty `<name>.none` for a tile without data. `state/tiles/` keeps the
/// marks, so a resume makes no tile again that it has. Gives the pixel digest of each file of the
/// version by path.
pub fn make(
    job: &mut Job,
    tiles: &[Tile],
    make: impl Fn(&Job, &str) -> Result<(), String>,
) -> Result<BTreeMap<String, String>, String> {
    let marks = job.state.join("tiles");
    fs::create_dir_all(&marks).map_err(|e| format!("{}: {e}", marks.display()))?;
    let mut batch = Vec::new();
    for Tile { name, files } in tiles {
        let none = format!("{name}.none");
        if files.iter().all(|(path, _)| job.done(path)) || marks.join(&none).exists() {
            continue;
        }
        let made = files.iter().all(|(path, mark)| marks.join(mark).exists() && job.work.join(path).exists());
        if !made {
            make(job, name)?;
            let written = files.iter().map(|(_, mark)| mark).chain([&none]);
            for mark in written.filter(|mark| job.work.join(mark).exists()) {
                fs::rename(job.work.join(mark), marks.join(mark)).map_err(|e| format!("{mark}: {e}"))?;
            }
            if marks.join(&none).exists() {
                continue;
            }
            if files.last().is_some_and(|(_, mark)| !marks.join(mark).exists()) {
                return Err(format!("the program of tile {name} wrote neither its files nor `{none}`"));
            }
        }
        batch.extend(files.iter().map(|(path, _)| path.clone()));
        if batch.len() >= BATCH {
            job.finish(&std::mem::take(&mut batch))?;
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
