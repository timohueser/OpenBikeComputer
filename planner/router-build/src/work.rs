//! The work folder and the sort-run size of the bake.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// The bytes of one sort run: 4 GiB, or `OBC_DATA_MEMORY_BYTES` when it is lower, and at least
/// 1 MiB. A run sorts in place in its column's mapped file, so this bounds the file pages that one
/// sort touches at once, not the bake's own memory. The floor keeps the merge heap small.
pub fn sort_run_bytes() -> usize {
    const MIN: u64 = 1 << 20;
    const MAX: u64 = 4 << 30;
    let set = std::env::var("OBC_DATA_MEMORY_BYTES").ok().map(|value| {
        value.parse::<u64>().unwrap_or_else(|_| panic!("OBC_DATA_MEMORY_BYTES is not a number of bytes: {value:?}"))
    });
    set.map_or(MAX, |bytes| bytes.clamp(MIN, MAX)) as usize
}

/// `OBC_DATA_WORK`, else `TMPDIR`, which `obc data` sets on every platform, else the temporary
/// folder of the system.
fn base() -> PathBuf {
    ["OBC_DATA_WORK", "TMPDIR"]
        .into_iter()
        .find_map(|name| std::env::var_os(name).filter(|value| !value.is_empty()))
        .map_or_else(std::env::temp_dir, PathBuf::from)
}

/// A new folder in `base`. The name holds the process ID, but a killed bake or another container
/// can have used it, so the name takes the first free number.
fn new_folder(base: &Path) -> std::io::Result<PathBuf> {
    fs::create_dir_all(base)?;
    for attempt in 0u64.. {
        let folder = base.join(format!("obc-router-build-{}-{attempt}", std::process::id()));
        match fs::create_dir(&folder) {
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            result => return result.map(|()| folder),
        }
    }
    unreachable!("a folder name is free before the numbers end")
}

/// The bake's folder while it holds a file, the name of the next file, and the number of files.
static FILES: Mutex<(Option<PathBuf>, u64, usize)> = Mutex::new((None, 0, 0));

/// A new empty file in the bake's own folder of the work folder, removed when dropped. The folder
/// exists while it holds a file, so the bake leaves nothing behind after an error or a panic.
pub struct File(PathBuf);

impl File {
    pub fn create() -> std::io::Result<Self> {
        let mut files = FILES.lock().unwrap_or_else(PoisonError::into_inner);
        let (folder, next, open) = &mut *files;
        if folder.is_none() {
            *folder = Some(new_folder(&base())?);
        }
        let path = folder.as_ref().expect("the folder exists").join(next.to_string());
        fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
        *next += 1;
        *open += 1;
        Ok(Self(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for File {
    fn drop(&mut self) {
        let mut files = FILES.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = fs::remove_file(&self.0);
        files.2 -= 1;
        if files.2 == 0 {
            if let Some(folder) = files.0.take() {
                let _ = fs::remove_dir(folder);
            }
        }
    }
}
