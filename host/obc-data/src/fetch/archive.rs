//! The archive mirror: whole datasets on a drive of their own. Each file is at the host and path of
//! its URL under `mirror/`, and a manifest under `manifest/` lists it with its size.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

use super::http::{self, Downloaded, Expect};
use crate::date;
use crate::store::{self, Store};

pub struct Archive {
    root: PathBuf,
    /// The files of every manifest by URL, read at the first lookup.
    listed: OnceLock<Result<HashMap<String, Listed>, String>>,
}

#[derive(Deserialize)]
struct Manifest {
    files: Vec<Listed>,
}

#[derive(Deserialize)]
struct Listed {
    url: String,
    size: u64,
    #[serde(default)]
    sha256: Option<String>,
}

impl Archive {
    pub fn new(root: PathBuf) -> Self {
        Self { root, listed: OnceLock::new() }
    }

    /// The archive in `OBC_DATA_ARCHIVE`, when it is set and not empty.
    pub fn from_env() -> Option<Self> {
        std::env::var_os("OBC_DATA_ARCHIVE").filter(|root| !root.is_empty()).map(|root| Self::new(root.into()))
    }

    /// Put the mirror's file of `url` into the store, as a download that meets `expect` would. `None`
    /// when the mirror has no such file, or no manifest lists it: a file without a manifest can be
    /// one that the mirror still downloads.
    pub fn import(&self, store: &Store, url: &str, expect: &Expect) -> Result<Option<Downloaded>, String> {
        let Some(path) = self.path(url) else { return Ok(None) };
        let listed = self.listed.get_or_init(|| read_manifests(&self.root.join("manifest")));
        let Some(listed) = listed.as_ref().map_err(Clone::clone)?.get(url) else { return Ok(None) };
        let Ok(metadata) = fs::metadata(&path).filter(|metadata| metadata.is_file()) else { return Ok(None) };
        let size = metadata.len();
        if size != listed.size {
            return Err(format!("{}: {size} bytes, but the manifest lists {}", path.display(), listed.size));
        }
        let modified = metadata.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
        let day = modified.map(|since| date::format((since.as_secs() / 86_400) as i64));
        if let (Some(day), Some(by)) = (day, expect.modified_by.filter(|by| day.as_deref() > Some(*by))) {
            return Err(format!("{} changed on {day}, after the version {by}", path.display()));
        }
        let part = store.partial(&format!("{}.mirror", http::key(url)));
        let parent = part.parent().ok_or("the partial path has no parent")?;
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        let _ = fs::remove_file(&part);
        // A hard link costs no space when the store and the mirror share a file system.
        if fs::hard_link(&path, &part).is_err() {
            store::check_free(&part, size)?;
            fs::copy(&path, &part).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        let (sha256, _) = store::hash_file(&part)?;
        if let Some(want) = expect.sha256.into_iter().chain(listed.sha256.as_deref()).find(|want| *want != sha256) {
            let _ = fs::remove_file(&part);
            return Err(format!("{}: the SHA-256 is {sha256}, not {want}", path.display()));
        }
        let object = store.insert(&part, &sha256)?;
        Ok(Some(Downloaded { object, sha256, size }))
    }

    /// `<root>/mirror/<host>/<path>` of an `https://` URL without a query, a fragment or a `.` or
    /// `..` segment.
    fn path(&self, url: &str) -> Option<PathBuf> {
        let rest = url.strip_prefix("https://").filter(|rest| !rest.contains(['?', '#']))?;
        let segments: Vec<&str> = rest.split('/').collect();
        let plain = segments.iter().all(|segment| !segment.is_empty() && *segment != "." && *segment != "..");
        plain.then(|| segments.iter().fold(self.root.join("mirror"), |path, segment| path.join(segment)))
    }
}

/// The files of every `*.json` manifest in `dir`, by URL. A manifest that cannot be read fails
/// every lookup, so a bad manifest never passes unseen.
fn read_manifests(dir: &Path) -> Result<HashMap<String, Listed>, String> {
    let mut listed = HashMap::new();
    let Ok(entries) = fs::read_dir(dir) else { return Ok(listed) };
    for entry in entries {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let manifest: Manifest = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        listed.extend(manifest.files.into_iter().map(|file| (file.url.clone(), file)));
    }
    Ok(listed)
}
