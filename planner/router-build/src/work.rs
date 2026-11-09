//! The work folder and the memory budget of the bake.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// The bytes that the bake's own memory may take: 4 GiB, or less when `OBC_DATA_MEMORY_BYTES` says so.
pub fn memory_budget() -> usize {
    const MAX: u64 = 4 << 30;
    let set = std::env::var("OBC_DATA_MEMORY_BYTES").ok().and_then(|bytes| bytes.parse::<u64>().ok());
    set.map_or(MAX, |bytes| bytes.min(MAX)) as usize
}

/// The name of the next file, and the number of files that exist.
static FILES: Mutex<(u64, usize)> = Mutex::new((0, 0));

/// An empty file in the bake's folder of the work folder, removed when dropped. The folder exists
/// while it holds a file, so the bake leaves nothing behind after an error or a panic.
pub struct File(PathBuf);

impl File {
    pub fn create() -> std::io::Result<Self> {
        let mut files = FILES.lock().unwrap_or_else(PoisonError::into_inner);
        let base = std::env::var_os("OBC_DATA_WORK").map_or_else(std::env::temp_dir, PathBuf::from);
        let folder = base.join(format!("obc-router-build-{}", std::process::id()));
        fs::create_dir_all(&folder)?;
        let path = folder.join(files.0.to_string());
        fs::File::create(&path)?;
        files.0 += 1;
        files.1 += 1;
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
        files.1 -= 1;
        if files.1 == 0 {
            let _ = self.0.parent().map(fs::remove_dir);
        }
    }
}
