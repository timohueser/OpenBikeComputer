//! Native landing place for verified assemblies.
//!
//! The wasm worker produces exactly the same bytes in every host. The website
//! hands each file to the browser downloader; desktop writes the files into one
//! newly-created folder under `Documents/OpenBikeComputer`. An opaque id keeps
//! the webview from turning this into a write-anywhere command.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Opened {
    pub id: u64,
    pub path: String,
}

#[derive(Default)]
pub struct Outputs {
    next: AtomicU64,
    open: Mutex<HashMap<u64, PathBuf>>,
}

impl Outputs {
    pub fn begin(&self, maps: &Path, name: &str) -> Result<Opened, String> {
        std::fs::create_dir_all(maps).map_err(|e| format!("{}: {e}", maps.display()))?;
        let base = folder_name(name);
        let path = (1..10_000)
            .find_map(|n| {
                let candidate = maps.join(if n == 1 { base.clone() } else { format!("{base}-{n}") });
                match std::fs::create_dir(&candidate) {
                    Ok(()) => Some(Ok(candidate)),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(e) => Some(Err(format!("{}: {e}", candidate.display()))),
                }
            })
            .ok_or_else(|| format!("{}: could not allocate an output folder", maps.display()))??;
        let id = self.next.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        self.open.lock().map_err(|_| "map output registry is poisoned".to_string())?.insert(id, path.clone());
        Ok(Opened { id, path: path.display().to_string() })
    }

    pub fn write(&self, id: u64, name: &str, bytes: &[u8]) -> Result<String, String> {
        let open = self.open.lock().map_err(|_| "map output registry is poisoned".to_string())?;
        let root = open.get(&id).ok_or_else(|| "that map output session is not open".to_string())?;
        write_file(root, name, bytes)
    }

    pub fn finish(&self, id: u64) -> Result<(), String> {
        self.take(id)?;
        Ok(())
    }

    pub fn discard(&self, id: u64) -> Result<(), String> {
        let mut open = self.open.lock().map_err(|_| "map output registry is poisoned".to_string())?;
        let path = open.get(&id).ok_or_else(|| "that map output session is not open".to_string())?;
        match std::fs::remove_dir_all(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("discard {}: {e}", path.display())),
        }
        open.remove(&id);
        Ok(())
    }

    fn take(&self, id: u64) -> Result<PathBuf, String> {
        self.open
            .lock()
            .map_err(|_| "map output registry is poisoned".to_string())?
            .remove(&id)
            .ok_or_else(|| "that map output session is not open".into())
    }
}

fn write_file(root: &Path, name: &str, bytes: &[u8]) -> Result<String, String> {
    validate_filename(name)?;
    let final_path = root.join(name);
    let part_path = root.join(format!(".{name}.part"));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part_path)
            .map_err(|e| format!("{}: {e}", part_path.display()))?;
        file.write_all(bytes).map_err(|e| format!("{}: {e}", part_path.display()))?;
        file.sync_all().map_err(|e| format!("{}: {e}", part_path.display()))?;
        std::fs::rename(&part_path, &final_path).map_err(|e| format!("{}: {e}", final_path.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part_path);
    }
    result?;
    Ok(final_path.display().to_string())
}

fn folder_name(name: &str) -> String {
    let cleaned: String =
        name.trim().chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-')).take(80).collect();
    if cleaned.is_empty() {
        "Map".into()
    } else {
        cleaned
    }
}

fn validate_filename(name: &str) -> Result<(), String> {
    let safe = !name.is_empty()
        && name != "."
        && name != ".."
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if safe {
        Ok(())
    } else {
        Err(format!("unsafe map output filename `{name}`"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_output_is_grouped_and_cannot_escape() {
        let maps = std::env::temp_dir().join(format!("obc-map-output-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&maps);
        let outputs = Outputs::default();
        let opened = outputs.begin(&maps, "Baden-Württemberg / tour").unwrap();
        let path = outputs.write(opened.id, "MS1.OBS", b"set").unwrap();
        assert!(Path::new(&path).starts_with(&maps));
        assert_eq!(std::fs::read(path).unwrap(), b"set");
        assert!(outputs.write(opened.id, "../escape", b"no").is_err());
        outputs.finish(opened.id).unwrap();
        assert!(outputs.write(opened.id, "MS1.OBS", b"late").is_err());
        let discarded = outputs.begin(&maps, "Discarded").unwrap();
        outputs.write(discarded.id, "MS1.OBS", b"partial").unwrap();
        outputs.discard(discarded.id).unwrap();
        assert!(!Path::new(&discarded.path).exists());
        let _ = std::fs::remove_dir_all(&maps);
    }
}
